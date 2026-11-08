import hashlib

SALT = b"salt"


def checksum_block(data):
    digest = hashlib.blake2b(data, digest_size=16)
    digest.update(SALT)
    return digest.finalize_reset()
