//! Throwaway probe: print the soft-object-path table location and hex-dump it.
use std::fs;

use uasset_parser::{Package, PackageSummary};

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe_softpath <file>");
    let bytes = fs::read(&path).expect("read file");
    if let Ok(package) = Package::parse(&bytes) {
        println!("resolved soft_object_paths:");
        for (i, p) in package.soft_object_paths.iter().enumerate() {
            println!("  [{i}] = {p:?}");
        }
    }
    let summary = PackageSummary::parse(&bytes).expect("parse summary");

    println!("package_name: {}", summary.package_name);
    println!("total_header_size: {}", summary.total_header_size);
    println!("names: count={} offset={}", summary.names.count, summary.names.offset.get());
    match summary.soft_object_paths {
        None => println!("soft_object_paths: <none>"),
        Some(table) => {
            let offset = table.offset.get() as usize;
            println!("soft_object_paths: count={} offset={}", table.count, offset);
            let end = (offset + 96).min(bytes.len());
            for i in (offset..end).step_by(16) {
                let mut line = format!("{i:5}: ");
                let mut ascii = String::new();
                for j in i..(i + 16).min(end) {
                    line.push_str(&format!("{:02x} ", bytes[j]));
                    let c = bytes[j];
                    ascii.push(if (32..127).contains(&c) { c as char } else { '.' });
                }
                println!("{line}  {ascii}");
            }
        }
    }
}
