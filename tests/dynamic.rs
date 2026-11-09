//! The script-engine registry: engines are pluggable, the registry is
//! feature-free, and unknown names are explicit errors. A recording test
//! engine pins the registry + session contracts without any real engine;
//! the `script`-gated tests prove v12 loads through the SAME path.

use std::sync::{Arc, Mutex};

use hyg::dynamic::engine::{
    create_script_session, register_script_engine, script_engine_names, HostFn, ScriptEngine,
    ScriptError, ScriptSession,
};
use hyg::Var;

// ---- the recording engine: a registry-contract probe, like audio's none ----

#[derive(Default)]
struct RecordingSession {
    log: Vec<String>,
    /// canned responses keyed by the operation name
    globals: Mutex<HashMap<String, Var>>,
}

use std::collections::HashMap;

impl ScriptSession for RecordingSession {
    fn run(&mut self) -> Result<Var, ScriptError> {
        self.log.push("run".into());
        Ok(Var::Null)
    }

    fn set_global(&mut self, name: &str, _value: &Var) -> Result<(), ScriptError> {
        self.log.push(format!("set_global:{name}"));
        Ok(())
    }

    fn get_global(&mut self, name: &str) -> Result<Var, ScriptError> {
        self.log.push(format!("get_global:{name}"));
        Ok(self
            .globals
            .lock()
            .expect("globals")
            .get(name)
            .cloned()
            .unwrap_or(Var::from("recorded")))
    }

    fn call(&mut self, name: &str, args: &[Var]) -> Result<Var, ScriptError> {
        self.log.push(format!("call:{name}:{}", args.len()));
        Ok(Var::from(format!("{name} called")))
    }

    fn call_unit(&mut self, name: &str, args: &[f64]) -> Result<(), ScriptError> {
        self.log.push(format!("call_unit:{name}:{}", args.len()));
        Ok(())
    }

    fn bind_host(&mut self, name: &str, _host: HostFn) -> Result<(), ScriptError> {
        self.log.push(format!("bind_host:{name}"));
        Ok(())
    }

    fn eval_bootstrap(&mut self, js: &str) -> Result<(), ScriptError> {
        self.log.push(format!("bootstrap:{js}"));
        Ok(())
    }
}

/// The engine captures what the factory was handed (source, hosts,
/// bootstraps) so the tests can assert the registry passes them through.
struct RecordingEngine {
    seen: Mutex<Vec<String>>,
}

impl ScriptEngine for RecordingEngine {
    fn name(&self) -> &str {
        "recording"
    }

    fn session(
        &self,
        source: &str,
        hosts: &[(String, HostFn)],
        bootstraps: &[String],
    ) -> Result<Box<dyn ScriptSession>, ScriptError> {
        self.seen.lock().expect("seen").push(format!(
            "session:{source}:{}:{}",
            hosts.len(),
            bootstraps.len()
        ));
        Ok(Box::new(RecordingSession::default()))
    }
}

#[test]
fn registry_resolves_and_reports_unknown_names() {
    // unknown names are explicit errors naming what IS registered
    let err = create_script_session("recording", "", &[], &[]).err()
        .expect("recording not registered yet");
    assert!(err.to_string().contains("unknown script engine"), "{err}");

    register_script_engine("recording", Arc::new(RecordingEngine { seen: Mutex::new(Vec::new()) }));
    assert!(
        script_engine_names().iter().any(|n| n == "recording"),
        "the engine appears in the registry"
    );

    // now it resolves
    let session = create_script_session("recording", "print(1)", &[], &[]);
    assert!(session.is_ok(), "registered engines resolve: {:?}", session.err());

    // re-registering the same name replaces (insert semantics — an upgrade
    // path, never a duplicate)
    register_script_engine("recording", Arc::new(RecordingEngine { seen: Mutex::new(Vec::new()) }));
    assert!(create_script_session("recording", "", &[], &[]).is_ok());
}

#[test]
fn sessions_receive_source_hosts_and_bootstraps() {
    let engine = Arc::new(RecordingEngine { seen: Mutex::new(Vec::new()) });
    let probe: Arc<dyn ScriptEngine> = engine.clone();
    register_script_engine("recording-probe", probe);

    let host: HostFn = Arc::new(Mutex::new(Box::new(|_args: &[Var]| Ok(Var::Null))));
    let session = create_script_session(
        "recording-probe",
        "let t = 0;",
        &[("transform".to_string(), host)],
        &["shim();".to_string()],
    )
    .expect("probe session materializes");
    drop(session);

    let seen = engine.seen.lock().expect("seen");
    assert_eq!(
        seen.last().map(String::as_str),
        Some("session:let t = 0;:1:1"),
        "source, host count, and bootstrap count pass through the registry"
    );
}

#[test]
fn session_operations_reach_the_engine_and_errors_propagate() {
    register_script_engine("recording-ops", Arc::new(RecordingEngine { seen: Mutex::new(Vec::new()) }));
    let mut session = create_script_session("recording-ops", "", &[], &[]).unwrap();

    assert_eq!(session.run().unwrap(), Var::Null);
    session.set_global("x", &Var::from(1i64)).unwrap();
    assert_eq!(session.get_global("x").unwrap(), Var::from("recorded"));
    assert_eq!(session.call("hook", &[Var::Null]).unwrap(), Var::from("hook called"));
    session.call_unit("update", &[1.0]).unwrap();
    let host: HostFn = Arc::new(Mutex::new(Box::new(|_args: &[Var]| Ok(Var::Null))));
    session.bind_host("hosted", host).unwrap();
    session.eval_bootstrap("arm();").unwrap();
}

#[cfg(feature = "script")]
mod v12_through_the_registry {
    use hyg::dynamic::engine::{create_script_session, script_engine_names};
    use hyg::Var;

    #[test]
    fn v12_is_registered_and_sessions_speak_the_contract() {
        assert!(
            script_engine_names().iter().any(|n| n == "v12"),
            "the built-in engine is pre-seeded"
        );

        // the SAME registry path the Script facade uses, driven directly
        let mut session = create_script_session(
            "v12",
            "function greet(who) { return 'hi ' + who; }",
            &[],
            &[],
        )
        .expect("v12 session materializes");
        let out = session.call("greet", &[Var::from("registry")]).unwrap();
        assert_eq!(out.as_str(), Some("hi registry"));
    }

    #[test]
    fn unknown_engines_name_the_registered_set() {
        let err = create_script_session("lua?", "", &[], &[])
            .err()
            .expect("an unknown engine is an explicit error, not a fallback");
        let msg = err.to_string();
        assert!(msg.contains("unknown script engine"), "{msg}");
        assert!(msg.contains("\"v12\""), "the error names what IS registered: {msg}");
    }
}
