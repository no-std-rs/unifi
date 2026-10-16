//! Optional host JPEG encoding, separate from the no-alloc Rust process.
pub fn jpeg<'a>(input: &[u8], quality: u8, output: &'a mut [u8]) -> Result<&'a [u8], &'static str> {
    use core::fmt::Write;
    let mut value = heapless::String::<3>::new();
    write!(value, "{quality}").unwrap();
    let (n, status) = crate::process::run(
        "/usr/bin/convert",
        &[
            "-limit", "memory", "32MiB", "-limit", "map", "0", "-limit", "disk", "0", "jpeg:-",
            "-quality", &value, "jpeg:-",
        ],
        "",
        input,
        output,
        30,
        false,
        crate::terminal::cancelled,
    )?;
    if status != 0 {
        return Err("JPEG encoding failed; --jpeg-quality needs /usr/bin/convert (ImageMagick)");
    }
    Ok(&output[..n])
}
