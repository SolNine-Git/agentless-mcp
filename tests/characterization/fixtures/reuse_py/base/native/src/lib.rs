const SALT: &[u8] = b"salt";

pub fn checksum_block(data: &[u8]) -> String {
    let mut digest = blake2b(data, 16);
    digest.update(SALT);
    digest.hexdigest()
}
