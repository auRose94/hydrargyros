//! Textures — pixel images the mesh pipeline samples (the bridge to glTF's
//! `baseColorTexture` and 2D sprites).
//!
//! A [`Texture`] is CPU-owned RGBA8 that renderers upload once and cache by
//! id, exactly like [`Mesh`](crate::game::mesh::Mesh). Sample mapping
//! follows glTF's convention: uv (0, 0) is the image's **top-left** — row
//! zero uploads first, so no flip is needed anywhere.

use std::sync::atomic::{AtomicU64, Ordering};

static TEXTURE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A CPU-side RGBA8 texture, shared as an `Arc` like meshes.
#[derive(Debug, Clone)]
pub struct Texture {
    id: u64,
    width: u32,
    height: u32,
    /// Row-major RGBA8, `width * height * 4` bytes, row 0 = image top.
    rgba: Vec<u8>,
}

impl Texture {
    /// A texture from raw RGBA8 pixels.
    ///
    /// # Errors
    /// `rgba.len() != width * height * 4`.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Texture, String> {
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(format!(
                "texture: {}×{} needs {expected} bytes, got {}",
                width,
                height,
                rgba.len()
            ));
        }
        Ok(Texture {
            id: TEXTURE_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            rgba,
        })
    }

    /// Decode an image file (PNG, JPEG, … — whatever the `image` crate
    /// reads) into an RGBA8 texture.
    ///
    /// # Errors
    /// The file can't be read or doesn't decode.
    #[cfg(feature = "image")]
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Texture, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("texture: {e}"))?;
        Texture::from_bytes(&bytes)
    }

    /// Decode image bytes (PNG, JPEG, …) into an RGBA8 texture.
    ///
    /// # Errors
    /// The bytes don't decode.
    #[cfg(feature = "image")]
    pub fn from_bytes(bytes: &[u8]) -> Result<Texture, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("texture: {e}"))?;
        let rgba = img.to_rgba8().into_raw();
        Texture::from_rgba(img.width(), img.height(), rgba)
    }

    /// The stable id (GPU backends cache uploads against it).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA8 pixels, row-major, row 0 = top.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// A 1×1 solid-color texture (the renderer's untextured fallback).
    pub fn white() -> Texture {
        Texture::from_rgba(1, 1, vec![255; 4]).expect("1×1 white always fits")
    }

    /// Convert a glTF-decoded image (already RGBA/RGB 8-bit) into a
    /// texture; 16-bit and float formats error — real models are 8-bit.
    ///
    /// # Errors
    /// The format isn't 8-bit RGB/RGBA.
    #[cfg(feature = "gltf")]
    pub fn from_gltf_data(data: gltf::image::Data) -> Result<Texture, String> {
        match data.format {
            gltf::image::Format::R8G8B8A8 => {
                Texture::from_rgba(data.width, data.height, data.pixels)
            }
            gltf::image::Format::R8G8B8 => {
                let mut rgba = Vec::with_capacity(data.pixels.len() / 3 * 4);
                for px in data.pixels.as_chunks::<3>().0 {
                    rgba.extend_from_slice(px);
                    rgba.push(255);
                }
                Texture::from_rgba(data.width, data.height, rgba)
            }
            other => Err(format!("texture: unsupported glTF image format {other:?}")),
        }
    }
}
