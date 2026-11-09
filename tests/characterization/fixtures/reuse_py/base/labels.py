import unicodedata


def decode_label(raw):
    text = raw.decode("utf-8", errors="replace")
    return unicodedata.normalize("NFC", text).strip().title()
