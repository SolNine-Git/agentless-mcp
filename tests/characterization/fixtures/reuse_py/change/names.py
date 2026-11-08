import unicodedata


def clean_label(data):
    words = []
    for word in unicodedata.normalize("NFC", data.decode("utf-8", "replace")).split():
        words.append(word.title())
    return " ".join(words)
