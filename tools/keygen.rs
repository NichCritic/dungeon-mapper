use ed25519_dalek::{SigningKey, Signer, VerifyingKey, Verifier, Signature};
use rand::rngs::OsRng;
use std::io::Read;

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 3 && args[1] == "--sign" {
        // Sign mode: keygen --sign <file> --key <keyfile_or_hex>
        let file_path = &args[2];
        let key_source = if args.len() >= 5 && args[3] == "--key" {
            args[4].clone()
        } else {
            // Try env var
            std::env::var("SIGNING_KEY").expect("Provide --key <file_or_hex> or set SIGNING_KEY env var")
        };

        // Read the secret key (either from file or direct hex)
        let key_hex = if std::path::Path::new(&key_source).exists() {
            std::fs::read_to_string(&key_source).unwrap().trim().to_string()
        } else {
            key_source
        };

        let key_bytes = hex_decode(&key_hex);
        let signing_key = SigningKey::from_bytes(key_bytes.as_slice().try_into().expect("Key must be 32 bytes"));

        let mut file_data = Vec::new();
        std::fs::File::open(file_path)
            .expect("Cannot open file")
            .read_to_end(&mut file_data)
            .expect("Cannot read file");

        let signature = signing_key.sign(&file_data);
        let sig_path = format!("{}.sig", file_path);
        std::fs::write(&sig_path, signature.to_bytes()).expect("Cannot write signature");
        eprintln!("Signed {} -> {}", file_path, sig_path);
    } else if args.len() >= 3 && args[1] == "--verify" {
        // Verify mode: keygen --verify <file> --pubkey <hex>
        let file_path = &args[2];
        let pubkey_hex = if args.len() >= 5 && args[3] == "--pubkey" {
            args[4].clone()
        } else {
            panic!("Usage: keygen --verify <file> --pubkey <hex>");
        };

        let pubkey_bytes = hex_decode(&pubkey_hex);
        let verifying_key = VerifyingKey::from_bytes(pubkey_bytes.as_slice().try_into().expect("Key must be 32 bytes")).unwrap();

        let mut file_data = Vec::new();
        std::fs::File::open(file_path)
            .expect("Cannot open file")
            .read_to_end(&mut file_data)
            .expect("Cannot read file");

        let sig_path = format!("{}.sig", file_path);
        let sig_bytes = std::fs::read(&sig_path).expect("Cannot read .sig file");
        let signature = Signature::from_bytes(sig_bytes.as_slice().try_into().expect("Signature must be 64 bytes"));

        match verifying_key.verify(&file_data, &signature) {
            Ok(()) => eprintln!("Signature valid."),
            Err(e) => {
                eprintln!("Signature INVALID: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Generate mode (default)
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key: VerifyingKey = (&signing_key).into();

        let secret_hex = hex_encode(signing_key.as_bytes());
        let public_hex = hex_encode(verifying_key.as_bytes());

        std::fs::write("signing-key.secret", &secret_hex).expect("Cannot write signing-key.secret");
        eprintln!("Secret key written to: signing-key.secret");
        eprintln!("Public key (set as GitHub secret UPDATE_PUBLIC_KEY):");
        println!("{}", public_hex);
    }
}
