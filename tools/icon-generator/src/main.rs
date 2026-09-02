use std::{fs, io::BufWriter, path::PathBuf};

fn png_bytes(rgba: &[u8], size: u32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(BufWriter::new(&mut bytes), size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(bytes)
}

fn render(tree: &resvg::usvg::Tree, size: u32) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("pixmap allocation")?;
    let scale = size as f32 / tree.size().width();
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap.take())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let svg = fs::read(root.join("assets/brand/devtoolbox.svg"))?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())?;
    let output = root.join("assets/app-icon");
    fs::create_dir_all(&output)?;

    let rgba = render(&tree, 1024)?;
    let png = png_bytes(&rgba, 1024)?;
    fs::write(output.join("devtoolbox.png"), &png)?;

    let mut directory = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 32, 48, 64, 128, 256] {
        let image = ico::IconImage::from_rgba_data(size, size, render(&tree, size)?);
        directory.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    let mut ico_file = BufWriter::new(fs::File::create(output.join("devtoolbox.ico"))?);
    directory.write(&mut ico_file)?;

    // Modern ICNS accepts a PNG-compressed 1024×1024 ic10 element.
    let total = 16 + png.len();
    let mut icns = Vec::with_capacity(total);
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&(total as u32).to_be_bytes());
    icns.extend_from_slice(b"ic10");
    icns.extend_from_slice(&((png.len() + 8) as u32).to_be_bytes());
    icns.extend_from_slice(&png);
    fs::write(output.join("devtoolbox.icns"), icns)?;
    Ok(())
}
