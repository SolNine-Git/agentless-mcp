"""Module docstring.

Spans lines.
"""


def documented(a):
    """Function docstring."""
    return a


def multi_docstring(a):
    """First line.

    More lines.
    """
    "A mid-body string statement."
    return a


def prefixed(a):
    b"bytes statement"
    f"f-string statement {a}"
    rb"raw bytes statement"
    u"unicode statement"
    return a


def trailing_comment(a):
    """Docstring with a comment."""  # trailing
    return a


def same_line(a):
    "éééééééé"; x = 1
    return x


def ascii_same_line(a):
    "eeeeeeee"; x = 1
    return x


def multiline_strings(a):
    text = """one
two
three"""
    other = f"""head
{a}
tail"""
    return text, other


def concatenated(a):
    "first" "second"
    return a


def separator(a):
    """Holds a line separator:  here."""
    return a


def after_separator(a):
    if a:
        return 1
    return 2



def after_formfeed(a):
    return a or 1
