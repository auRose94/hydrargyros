//! Texture unit tests (no GPU): raw-pixel validation, file decoding,
//! and the glTF image-data conversion.

use hydrargyros::game::texture::Texture;

/// `from_rgba` demands exactly `width * height * 4` bytes.
#[test]
fn from_rgba_validates_length() {
    assert!(Texture::from_rgba(2, 2, vec![0; 15]).is_err());
    assert!(Texture::from_rgba(2, 2, vec![0; 17]).is_err());
    let tex = Texture::from_rgba(2, 2, vec![1; 16]).expect("16 bytes for 2×2");
    assert_eq!((tex.width(), tex.height()), (2, 2));
    assert_eq!(tex.rgba().len(), 16);
    // distinct textures get distinct ids (GPU backends cache against them)
    let other = Texture::from_rgba(1, 1, vec![2; 4]).unwrap();
    assert_ne!(tex.id(), other.id());
}

/// A 1×1 solid white texture — the renderer's untextured fallback.
#[test]
fn white_is_one_opaque_pixel() {
    let tex = Texture::white();
    assert_eq!((tex.width(), tex.height()), (1, 1));
    assert_eq!(tex.rgba(), &[255, 255, 255, 255]);
}

/// `from_file` decodes real image files (PNG via the `image` feature).
#[cfg(feature = "image")]
#[test]
fn from_file_decodes_a_png() {
    let path =
        std::env::temp_dir().join(format!("hydrargyros-tex-test-{}.png", std::process::id()));
    image::save_buffer(
        &path,
        &[255u8, 0, 0, 255, 0, 255, 0, 255],
        2,
        1,
        image::ExtendedColorType::Rgba8,
    )
    .expect("png write");
    let tex = Texture::from_file(&path).expect("png decode");
    std::fs::remove_file(&path).ok();
    assert_eq!((tex.width(), tex.height()), (2, 1));
    assert_eq!(&tex.rgba()[..4], &[255, 0, 0, 255]);
    assert_eq!(&tex.rgba()[4..], &[0, 255, 0, 255]);
}

/// glTF's decoded image data converts: RGB gains an opaque alpha channel.
#[cfg(feature = "gltf")]
#[test]
fn gltf_rgb_data_gains_alpha() {
    let data = gltf::image::Data {
        pixels: vec![10, 20, 30, 40, 50, 60],
        format: gltf::image::Format::R8G8B8,
        width: 2,
        height: 1,
    };
    let tex = Texture::from_gltf_data(data).expect("rgb converts");
    assert_eq!((tex.width(), tex.height()), (2, 1));
    assert_eq!(tex.rgba(), &[10, 20, 30, 255, 40, 50, 60, 255]);

    // exotic formats error instead of misinterpreting bytes
    let data = gltf::image::Data {
        pixels: vec![0; 8],
        format: gltf::image::Format::R16G16B16A16,
        width: 1,
        height: 1,
    };
    assert!(Texture::from_gltf_data(data).is_err());
}
