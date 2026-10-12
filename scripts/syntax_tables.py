"""Write crates/syntax/languages/*.toml from the Python language tables.

Development-time only. Run it from the repository root, in the project environment:

    uv run --frozen python scripts/syntax_tables.py

With --check, the script compares its output with the committed files and writes nothing.
tests/unit/test_rust_language_tables.py runs the same comparison under pytest.
"""

import json
import sys
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path

from fixture_files import write_or_check_files

from agentless_mcp.core import clones, complexity, grammars, normalize
from agentless_mcp.core.extractor import (
    COMMENT_NODE_TYPES,
    INDENT_BLOCK_NODE_TYPES,
    TreeSitterExtractor,
)

ROOT = Path(__file__).resolve().parents[1]
LANGUAGES = ROOT / "crates" / "syntax" / "languages"
INLINE = 90

# Each Python rule function, by the name of the Rust rule that ports it.
NAME_RULES: Mapping[Callable[..., object], str] = {
    complexity._name_field: "field",
    complexity._c_name: "c_declarator",
    complexity._cpp_name: "cpp_declarator",
    complexity._rust_name: "rust_impl",
}
BINDING_RULES: Mapping[Callable[..., object], str] = {
    complexity._javascript_binding: "javascript",
    complexity._typescript_binding: "typescript",
    complexity._lua_binding: "lua",
}
OWNER_RULES: Mapping[Callable[..., object], str] = {complexity._go_receiver_type: "go_receiver"}
CONDITIONS: Mapping[Callable[..., object], str] = {
    complexity._has_named_child: "has_named_child",
    complexity._is_not_default_case: "not_default_case",
    complexity._is_counted_rust_arm: "counted_rust_arm",
    complexity._has_condition: "has_condition",
    complexity._is_counted_ruby_arm: "counted_ruby_arm",
    complexity._is_counted_case_item: "counted_case_item",
}
SLOC_RULES: Mapping[Callable[..., object], str] = {
    complexity._leaf_sloc_lines: "leaves",
    complexity._python_sloc_lines: "python_tokens",
}
SKIP_RULES: Mapping[Callable[..., object], str] = {clones._is_type_checking_block: "type_checking"}


def main() -> int:
    """Write the language files, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], LANGUAGES, contents)


def contents() -> dict[Path, bytes]:
    """Return the bytes of each language file, by path."""
    return {
        LANGUAGES / f"{language}.toml": render(language).encode("utf-8")
        for language in grammars.ALL_LANGUAGES
    }


def render(language: str) -> str:
    lines = [
        f"name = {string(language)}",
        f"tier = {1 if grammars.is_tier1(language) else 2}",
        f"extensions = {array(extensions(language))}",
        f"probe = {string(grammars._PROBE_SAMPLES[language])}",
        f"probe_root = {string(probe_root(language))}",
        f"comments = {array(present(language, COMMENT_NODE_TYPES))}",
    ]
    if language in complexity.RULES:
        lines += measured(language)
    return "\n".join(lines) + "\n"


def extensions(language: str) -> list[str]:
    return [
        suffix
        for suffix, name in TreeSitterExtractor.SUPPORTED_EXTENSIONS.items()
        if name == language
    ]


def probe_root(language: str) -> str:
    sample = grammars._PROBE_SAMPLES[language].encode("utf-8")
    return str(grammars.get_parser(language).parse(sample).root_node.type)


def present(language: str, names: Iterable[str]) -> list[str]:
    grammar = grammars.get_language(language)
    return sorted(
        name
        for name in names
        if grammar.id_for_node_kind(name, True) is not None
        or grammar.id_for_node_kind(name, False) is not None
    )


def measured(language: str) -> list[str]:
    rules = complexity.RULES[language]
    lines = [f"sloc = {string(SLOC_RULES[rules.sloc_lines])}"]
    drop = normalize._STATEMENT_DROPS.get(language)
    if drop is not None:
        if drop is not normalize._is_python_docstring:
            message = f"{language}: an unknown statement drop {drop.__name__}"
            raise ValueError(message)
        lines.append(f"statement_parents = {array(normalize.PYTHON_STATEMENT_PARENTS)}")
    return lines + units(rules.units) + decisions(rules.decisions) + clone_tables(language)


def units(rules: complexity.UnitRules) -> list[str]:
    lines = [
        "",
        "[units]",
        f"units = {array(rules.units)}",
        f"scopes = {array(rules.scopes)}",
        f"bound = {array(rules.bound)}",
    ]
    if rules.descend is not None:
        lines.append(f"descend = {array(rules.descend)}")
    lines.append(f"name = {string(NAME_RULES[rules.name_of])}")
    if rules.binding_of is not None:
        lines.append(f"binding = {string(BINDING_RULES[rules.binding_of])}")
    if rules.owner_of is not None:
        lines.append(f"owner = {string(OWNER_RULES[rules.owner_of])}")
    return lines


def decisions(rules: complexity.DecisionRules) -> list[str]:
    conditional = {kind: string(CONDITIONS[test]) for kind, test in rules.conditional.items()}
    fields = {kind: array(spellings) for kind, spellings in rules.field_operators.items()}
    tokens = {kind: array(spellings) for kind, spellings in rules.token_operators.items()}
    return [
        "",
        "[decisions]",
        f"points = {array(rules.points)}",
        *table("decisions.conditional", conditional),
        *table("decisions.field_operators", fields),
        *table("decisions.token_operators", tokens),
    ]


def clone_tables(language: str) -> list[str]:
    candidates = clones.CANDIDATES[language]
    lines = [
        "",
        "[clones]",
        f"functions = {array(candidates.functions)}",
        f"blocks = {array(candidates.blocks)}",
    ]
    if candidates.skip is not clones._never:
        lines.append(f"skip = {string(SKIP_RULES[candidates.skip])}")
    lines += [
        f"identifiers = {array(normalize.CLONE_IDENTIFIERS[language])}",
        f"indent_blocks = {array(INDENT_BLOCK_NODE_TYPES.get(language, ()))}",
    ]
    classes: dict[str, list[str]] = {}
    for kind, token in normalize.CLONE_LITERALS[language].items():
        classes.setdefault(token, []).append(kind)
    return lines + table(
        "clones.literals", {token: array(kinds) for token, kinds in classes.items()}
    )


def table(name: str, entries: Mapping[str, str]) -> list[str]:
    return ["", f"[{name}]", *(f"{string(key)} = {entries[key]}" for key in sorted(entries))]


def array(items: Iterable[str]) -> str:
    values = [string(item) for item in sorted(items)]
    inline = "[" + ", ".join(values) + "]"
    if len(inline) <= INLINE:
        return inline
    return "[\n" + "".join(f"    {value},\n" for value in values) + "]"


def string(text: str) -> str:
    return json.dumps(text)


if __name__ == "__main__":
    sys.exit(main())
