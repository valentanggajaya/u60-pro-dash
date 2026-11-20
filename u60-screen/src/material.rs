use anyhow::{Result, ensure};
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};
use std::io::Cursor;

// Decode the embedded textures once with our existing PNG dependency. Share
// them across every surface without enabling Slint's general image decoders
// or changing the embedded fonts to compile-time rasterized glyphs.
pub fn install(ui: &crate::Screen) -> Result<()> {
    let palette = ui.global::<crate::Palette>();
    palette.set_material_light(decode(include_bytes!("../assets/material-light.png"))?);
    palette.set_material_dark(decode(include_bytes!("../assets/material-dark.png"))?);
    // Full-screen night wallpaper behind the dark theme's translucent cards.
    palette.set_wallpaper_dark(decode_sized(include_bytes!("../assets/wallpaper-dark.png"), 320, 480)?);
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<Image> {
    decode_sized(bytes, 320, 180)
}

fn decode_sized(bytes: &[u8], width: u32, height: u32) -> Result<Image> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info()?;
    let info = reader.info();
    ensure!(
        info.width == width
            && info.height == height
            && info.bit_depth == png::BitDepth::Eight
            && info.color_type == png::ColorType::Rgb,
        "invalid embedded material texture"
    );
    let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(width, height);
    reader.next_frame(pixels.make_mut_bytes())?;
    Ok(Image::from_rgb8(pixels))
}
