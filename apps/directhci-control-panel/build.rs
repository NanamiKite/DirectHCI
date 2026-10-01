use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=directhci.rc");
    println!("cargo:rerun-if-changed=../../assets/directhci.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");

    if !env::var("TARGET")
        .expect("Cargo provides TARGET")
        .ends_with("-windows-gnu")
    {
        return;
    }

    let windres = env::var_os("WINDRES").unwrap_or_else(|| {
        if cfg!(windows) {
            "windres.exe".into()
        } else {
            "x86_64-w64-mingw32-windres".into()
        }
    });
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("directhci-icon.o");
    let status = Command::new(&windres)
        .args([
            "--input-format=rc",
            "--output-format=coff",
            "directhci.rc",
            "-o",
        ])
        .arg(&output)
        .status()
        .unwrap_or_else(|error| panic!("run {windres:?} to embed DirectHCI icon: {error}"));
    assert!(
        status.success(),
        "{windres:?} failed to compile the DirectHCI icon"
    );
    println!("cargo:rustc-link-arg={}", output.display());
}
