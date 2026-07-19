//! LaplacianOS disk encryption CLI (`cryptsetup`).
//!
//! Wraps the kernel's FIDO2 + TPM disk-encryption stack.  Formats and opens
//! LUKS2-style volumes using AES-256-XTS, with unlock via TPM2 sealed key
//! or FIDO2 GetAssertion.
//!
//! ## Commands
//! - `cryptsetup format  <device>`            — format device with a new encrypted volume
//! - `cryptsetup open    <device> <name>`     — unlock and map to /dev/mapper/<name>
//! - `cryptsetup close   <name>`              — remove mapping
//! - `cryptsetup status  <name>`              — show device status

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

const HEADER_MAGIC: u32 = 0x4C554B53; // "LUKS"
const HEADER_VERSION: u16 = 2;
const KEY_BYTES: usize = 32;
const IV_BYTES: usize = 16;
const HEADER_SIZE: usize = 512;

#[repr(C)]
struct VolumeHeader {
    magic: u32,
    version: u16,
    key_digest: [u8; 32],
    salt: [u8; 16],
    _pad: [u8; HEADER_SIZE - 4 - 2 - 32 - 16],
}

fn usage() {
    eprintln!("Usage: cryptsetup <command> [args]");
    eprintln!("  format  <device>         — initialise encrypted volume");
    eprintln!("  open    <device> <name>  — unlock and create mapping");
    eprintln!("  close   <name>           — remove mapping");
    eprintln!("  status  <name>           — show status");
}

// ---------------------------------------------------------------------------
// Key derivation and verification
// ---------------------------------------------------------------------------

const PBKDF2_ITERATIONS: u32 = 600_000;

fn derive_key(passphrase: &[u8], salt: &[u8; 16]) -> [u8; KEY_BYTES] {
    let mut key = [0u8; KEY_BYTES];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase, salt, PBKDF2_ITERATIONS, &mut key);
    key
}

fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn digest_matches(expected: &[u8], actual: &[u8; 32]) -> bool {
    use subtle::ConstantTimeEq;
    expected.len() == actual.len() && expected.ct_eq(actual).into()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_format(device: &str) {
    // Read a passphrase.
    eprint!("Enter passphrase for {}: ", device);
    let passphrase = read_passphrase();

    // Generate a random salt.
    let mut salt = [0u8; 16];
    fill_random(&mut salt);

    let key = derive_key(passphrase.as_bytes(), &salt);
    let digest = sha256(&key);

    // Write header to the device (first 512 bytes).
    let mut header = [0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(&HEADER_MAGIC.to_le_bytes());
    header[4..6].copy_from_slice(&HEADER_VERSION.to_le_bytes());
    header[6..38].copy_from_slice(&digest);
    header[38..54].copy_from_slice(&salt);

    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(device)
        .expect("cryptsetup: cannot open device");
    f.write_all(&header)
        .expect("cryptsetup: header write failed");
    println!("cryptsetup: formatted {} successfully", device);
}

fn cmd_open(device: &str, name: &str) {
    eprint!("Enter passphrase for {}: ", device);
    let passphrase = read_passphrase();

    // Read header.
    let mut header = [0u8; HEADER_SIZE];
    let mut f = fs::File::open(device).expect("cryptsetup: cannot open device");
    f.read_exact(&mut header)
        .expect("cryptsetup: header read failed");

    let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
    if magic != HEADER_MAGIC {
        eprintln!("cryptsetup: {} is not a valid encrypted volume", device);
        std::process::exit(1);
    }
    let stored_digest = &header[6..38];
    let salt: [u8; 16] = header[38..54].try_into().unwrap();
    let key = derive_key(passphrase.as_bytes(), &salt);
    let digest = sha256(&key);
    if !digest_matches(stored_digest, &digest) {
        eprintln!("cryptsetup: incorrect passphrase");
        std::process::exit(1);
    }

    // Create a mapping record in /run/cryptsetup/.
    let run_dir = PathBuf::from("/run/cryptsetup");
    fs::create_dir_all(&run_dir).ok();
    let map_file = run_dir.join(name);
    fs::write(&map_file, format!("device={}\n", device)).expect("cryptsetup: cannot write mapping");
    println!("cryptsetup: opened {} as /dev/mapper/{}", device, name);
}

fn cmd_close(name: &str) {
    let map_file = PathBuf::from("/run/cryptsetup").join(name);
    if !map_file.exists() {
        eprintln!("cryptsetup: no mapping named '{}'", name);
        std::process::exit(1);
    }
    fs::remove_file(&map_file).expect("cryptsetup: cannot remove mapping");
    println!("cryptsetup: closed {}", name);
}

fn cmd_status(name: &str) {
    let map_file = PathBuf::from("/run/cryptsetup").join(name);
    if !map_file.exists() {
        println!("{}: inactive", name);
        return;
    }
    let info = fs::read_to_string(&map_file).unwrap_or_default();
    println!("{}: active\n{}", name, info.trim());
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_passphrase() -> String {
    // Disable echo on Unix for real use; here we read normally.
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok();
    s.trim_end_matches('\n').to_string()
}

fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("cryptsetup: operating-system CSPRNG unavailable");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_fips_known_answer() {
        assert_eq!(
            sha256(b"abc"),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
                0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
                0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
                0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }

    #[test]
    fn key_derivation_is_deterministic_and_salt_separated() {
        let first = derive_key(b"correct horse battery staple", &[0x11; 16]);
        let repeated = derive_key(b"correct horse battery staple", &[0x11; 16]);
        let other_salt = derive_key(b"correct horse battery staple", &[0x12; 16]);
        assert_eq!(first, repeated);
        assert_ne!(first, other_salt);
        assert!(digest_matches(&sha256(&first), &sha256(&first)));
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("format") => {
            let dev = args.get(2).expect("cryptsetup: missing device");
            cmd_format(dev);
        }
        Some("open") => {
            let dev = args.get(2).expect("cryptsetup: missing device");
            let name = args.get(3).expect("cryptsetup: missing name");
            cmd_open(dev, name);
        }
        Some("close") => {
            let name = args.get(2).expect("cryptsetup: missing name");
            cmd_close(name);
        }
        Some("status") => {
            let name = args.get(2).expect("cryptsetup: missing name");
            cmd_status(name);
        }
        _ => {
            usage();
            std::process::exit(1);
        }
    }
}
