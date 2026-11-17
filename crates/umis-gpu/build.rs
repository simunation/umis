//! With feature `cuda`: compiles `cuda/*.cu` to a cubin (native SASS) with nvcc (`NVCC`, else
//! `/usr/local/cuda/bin/nvcc` or `nvcc` on PATH) for `sm_86` (override: `UMIS_CUDA_ARCH`).
//! A cubin runs on any driver of the same CUDA major version (minor-version compatibility);
//! PTX would need a driver at least as new as the toolkit.
fn main() {
    println!("cargo:rerun-if-changed=cuda");
    println!("cargo:rerun-if-env-changed=NVCC");
    println!("cargo:rerun-if-env-changed=UMIS_CUDA_ARCH");
    if std::env::var_os("CARGO_FEATURE_CUDA").is_none() {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let nvcc = std::env::var("NVCC").unwrap_or_else(|_| {
        if std::path::Path::new("/usr/local/cuda/bin/nvcc").exists() {
            "/usr/local/cuda/bin/nvcc".into()
        } else {
            "nvcc".into()
        }
    });
    let arch = std::env::var("UMIS_CUDA_ARCH").unwrap_or_else(|_| "sm_86".into());
    for name in ["lde", "resident"] {
        let status = std::process::Command::new(&nvcc)
            .args(["-cubin", "-O3", &format!("-arch={arch}"), &format!("cuda/{name}.cu"), "-o"])
            .arg(out.join(format!("{name}.cubin")))
            .status()
            .expect("run nvcc");
        assert!(status.success(), "nvcc failed on cuda/{name}.cu");
    }
}
