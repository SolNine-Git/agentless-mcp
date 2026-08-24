"""The catalogue record every other catalogue module reads."""

from dataclasses import dataclass

CATALOGUE_NAME = "spring"
MAX_TITLE_CHARS = 64


@dataclass(frozen=True)
class Product:
    """One saleable product in the catalogue."""

    code: str
    title: str
    band: str

    def short_title(self) -> str:
        """Return the title cut to the catalogue's column width."""
        if len(self.title) <= MAX_TITLE_CHARS:
            return self.title
        return self.title[: MAX_TITLE_CHARS - 3] + "..."


def parse_product(row: dict[str, str]) -> Product:
    """Build a product from one catalogue row, refusing a missing field."""
    for field in ("code", "title", "band"):
        if field not in row:
            message = f"catalogue row is missing {field}"
            raise KeyError(message)
    return Product(code=row["code"], title=row["title"], band=row["band"])
