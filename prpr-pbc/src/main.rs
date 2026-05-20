use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use prpr::{
    bin::BinaryWriter,
    core::ChartExtra,
    fs::FileSystem,
    parse::{infer_chart_format_bytes, parse_chart_bytes, ParseOptions},
};
use std::{
    any::Any,
    fs::File,
    io::BufWriter,
};

const HELP: &str = "
Usage: prpr-pbc [options] input output

Options:
    -h, --help  Display this message
";

struct DummyFileSystem;
#[async_trait]
impl FileSystem for DummyFileSystem {
    async fn load_file(&mut self, _path: &str) -> Result<Vec<u8>> {
        bail!("Not implemented");
    }
    async fn exists(&mut self, _path: &str) -> Result<bool> {
        Ok(false)
    }
    fn list_root(&self) -> Result<Vec<String>> {
        Ok(vec![])
    }
    fn clone_box(&self) -> Box<dyn FileSystem> {
        Box::new(DummyFileSystem)
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

fn main() -> Result<()> {
    let iter = std::env::args().skip(1);
    let mut input = None;
    let mut output = None;
    for arg in iter {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{}", HELP.trim());
                return Ok(());
            }
            _ => {
                if input.is_none() {
                    input = Some(arg);
                } else if output.is_none() {
                    output = Some(arg);
                } else {
                    bail!("Too many arguments");
                }
            }
        }
    }

    let input = input.ok_or_else(|| anyhow!("Missing input"))?;
    let output = output.ok_or_else(|| anyhow!("Missing output"))?;

    let bytes = std::fs::read(input).context("Failed to read chart")?;
    let format = infer_chart_format_bytes(None, &bytes);

    let mut fs = Box::new(DummyFileSystem);
    let extra = ChartExtra::default();
    let chart = pollster::block_on(parse_chart_bytes(&bytes, format, fs.as_mut(), extra, ParseOptions::default()))?;

    let output = BufWriter::new(File::create(output)?);
    let mut w = BinaryWriter::new(output);
    w.write(&chart)?;

    Ok(())
}
