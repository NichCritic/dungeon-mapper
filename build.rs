fn main() {
    let pubkey = std::env::var("UPDATE_PUBLIC_KEY")
        .unwrap_or_else(|_| "0".repeat(64));
    println!("cargo:rustc-env=UPDATE_PUBLIC_KEY={}", pubkey);
}
