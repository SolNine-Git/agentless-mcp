"""Settings: load has imported late_b in its body since the base (positive, return_path)."""


def load():
    import late_b

    return late_b.VALUE


def name():
    return "late"
