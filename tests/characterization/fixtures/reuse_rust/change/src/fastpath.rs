const SALT: &[u8] = b"salt";

pub fn checksum_block(data: &[u8]) -> Vec<u8> {
    let mut digest = blake2b(data, 16);
    digest.update(SALT);
    digest.finalize_reset()
}
