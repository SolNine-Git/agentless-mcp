"""Per-unit cyclomatic complexity, SLOC, mass, and erosion for the tier-1 languages."""

import bisect
import hashlib
import math
import tokenize
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, field
from functools import cache
from types import MappingProxyType
from typing import Final

from tree_sitter import Node, Query, QueryCursor

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import COMMENT_NODE_TYPES, error_nodes
from agentless_mcp.core.normalize import PYTHON_STATEMENT_PARENTS, node_stream, string_prefix
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import UnmeasurableSource

DEFAULT_THRESHOLD: Final = 10

NameOf = Callable[[Node], "Node | None"]
SlocLines = Callable[[], tuple[int, ...]]


@dataclass(frozen=True)
class Unit:
    """One named callable's metrics in one source text."""

    qualname: str
    path: str
    start_byte: int
    end_byte: int
    start_line: int
    end_line: int
    body_key: str
    cc: int
    sloc: int

    @property
    def mass(self) -> float:
        """Return cc times the square root of sloc."""
        return self.cc * math.sqrt(self.sloc)


def _name_field(node: Node) -> Node | None:
    return node.child_by_field_name("name")


def _no_operators() -> Mapping[str, frozenset[str]]:
    return MappingProxyType({})


def _no_predicates() -> Mapping[str, Callable[[Node], bool]]:
    return MappingProxyType({})


@dataclass(frozen=True)
class UnitRules:
    """How one language's units, their names, and their qualname scopes are found."""

    units: frozenset[str]
    scopes: frozenset[str] = frozenset()
    bound: frozenset[str] = frozenset()
    reads: frozenset[str] = frozenset()
    descend: frozenset[str] | None = None
    name_of: NameOf = _name_field
    binding_of: NameOf | None = None
    owner_of: NameOf | None = None


@dataclass(frozen=True)
class DecisionRules:
    """What one language counts as a decision point."""

    points: frozenset[str]
    conditional: Mapping[str, Callable[[Node], bool]] = field(default_factory=_no_predicates)
    field_operators: Mapping[str, frozenset[str]] = field(default_factory=_no_operators)
    token_operators: Mapping[str, frozenset[str]] = field(default_factory=_no_operators)


@dataclass(frozen=True)
class LanguageRules:
    """One tier-1 language's unit, decision, and SLOC rules."""

    units: UnitRules
    decisions: DecisionRules
    sloc_lines: Callable[[str, Node], frozenset[int]]


def measure(source: str, path: str, language: str) -> tuple[Unit, ...]:
    """Return every unit of source in source order, or raise UnmeasurableSource."""
    rules = _rules_for(path, language)
    root = grammars.get_parser(language).parse(source.encode("utf-8")).root_node
    return _measured(root, path, language, rules, lambda: sloc_line_numbers(root, source, language))


def measure_tree(root: Node, path: str, language: str, lines: SlocLines) -> tuple[Unit, ...]:
    """Return what `measure` returns, from the root of its UTF-8 parse and its SLOC lines."""
    return _measured(root, path, language, _rules_for(path, language), lines)


def sloc_line_numbers(root: Node, source: str, language: str) -> tuple[int, ...]:
    """Return the sorted SLOC lines of a parsed tier-1 source, or raise the tokenizer's error."""
    return tuple(sorted(RULES[language].sloc_lines(source, root)))


def erosion(units: Iterable[Unit], threshold: int = DEFAULT_THRESHOLD) -> float:
    """Return the share of mass held by units with cc above threshold, 0.0 for no mass."""
    measured = tuple(units)
    total = sum(unit.mass for unit in measured)
    if not total:
        return 0.0
    return sum(unit.mass for unit in measured if unit.cc > threshold) / total


def _rules_for(path: str, language: str) -> LanguageRules:
    rules = RULES.get(language)
    if rules is None:
        message = MESSAGES.complexity_language_unsupported.format(
            path=path, language=language, languages=", ".join(RULES)
        )
        raise UnmeasurableSource(message)
    return rules


def _measured(
    root: Node, path: str, language: str, rules: LanguageRules, sloc: SlocLines
) -> tuple[Unit, ...]:
    if root.has_error:
        first = next(error_nodes(root))
        message = MESSAGES.complexity_does_not_parse.format(path=path, line=_first_line(first))
        raise UnmeasurableSource(message)

    try:
        lines = sloc()
    except (tokenize.TokenError, SyntaxError) as exc:
        message = MESSAGES.complexity_tokenizer_rejects.format(
            path=path, detail=_tokenizer_detail(exc)
        )
        raise UnmeasurableSource(message) from exc

    return tuple(
        Unit(
            qualname=qualname,
            path=path,
            start_byte=node.start_byte,
            end_byte=node.end_byte,
            start_line=_first_line(node),
            end_line=_last_line(node),
            body_key=hashlib.sha256(
                node_stream(node, language, omit=name).encode("utf-8")
            ).hexdigest(),
            cc=1 + _decisions(node, rules.decisions),
            sloc=_lines_within(lines, _first_line(node), _last_line(node)),
        )
        for node, qualname, name in _walk_units(root, rules.units)
    )


def _tokenizer_detail(exc: tokenize.TokenError | SyntaxError) -> str:
    reason = exc.args[0] if exc.args else ""
    return f"{type(exc).__name__}: {reason}"


# Indexed, never `.row`: that attribute segfaults under Python 3.10 with
# py-tree-sitter 0.26, and the 3.10 CI row runs these paths.
def _first_line(node: Node) -> int:
    return node.start_point[0] + 1


def _last_line(node: Node) -> int:
    return node.end_point[0] + 1


def _lines_within(lines: tuple[int, ...], start: int, end: int) -> int:
    return bisect.bisect_right(lines, end) - bisect.bisect_left(lines, start)


def _walk_units(root: Node, rules: UnitRules) -> list[tuple[Node, str, Node]]:
    found: list[tuple[Node, str, Node]] = []
    stack: list[tuple[Node, str]] = [(root, "")]
    while stack:
        node, prefix = stack.pop()
        entered: list[tuple[Node, str]] = []
        for child in node.named_children:
            name = _unit_name(child, rules)
            if name is not None:
                qualname = _join(prefix, _owner_text(child, rules), _name_text(name))
                found.append((child, qualname, name))
                entered.append((child, qualname))
            elif child.type in rules.scopes:
                scope = rules.name_of(child)
                entered.append(
                    (child, prefix if scope is None else _join(prefix, _name_text(scope)))
                )
            elif rules.descend is None or child.type in rules.descend:
                entered.append((child, prefix))
        stack.extend(reversed(entered))
    found.sort(key=lambda item: item[0].start_byte)
    return found


def _unit_name(node: Node, rules: UnitRules) -> Node | None:
    if node.type in rules.units:
        name = rules.name_of(node)
    elif node.type in rules.bound:
        name = rules.name_of(node)
        if name is None and rules.binding_of is not None:
            name = rules.binding_of(node)
    else:
        return None
    if name is None or not _name_text(name):
        return None
    return name


def _owner_text(node: Node, rules: UnitRules) -> str:
    owner = None if rules.owner_of is None else rules.owner_of(node)
    return "" if owner is None else _name_text(owner)


def _join(*parts: str) -> str:
    return ".".join(part for part in parts if part)


def _text(node: Node) -> str:
    return (node.text or b"").decode("utf-8")


def _name_text(node: Node) -> str:
    if node.type == "string":
        text = "".join(
            _text(part) for part in node.named_children if part.type == "string_fragment"
        )
    else:
        text = _text(node)
    return text.replace("::", ".")


def _first_of_type(node: Node, wanted: str) -> Node | None:
    stack = [node]
    while stack:
        current = stack.pop()
        if current.type == wanted:
            return current
        stack.extend(reversed(current.children))
    return None


def _decisions(unit: Node, rules: DecisionRules) -> int:
    count = 0
    stack = [unit]
    while stack:
        node = stack.pop()
        stack.extend(node.children)
        # Ruby spells keywords such as `if` and `when` like the named nodes
        # they open, so only named nodes are weighed.
        if node.is_named:
            count += _weight(node, rules)
    return count


def _weight(node: Node, rules: DecisionRules) -> int:
    weight = int(node.type in rules.points)
    predicate = rules.conditional.get(node.type)
    if predicate is not None and predicate(node):
        weight += 1
    spellings = rules.field_operators.get(node.type)
    if spellings is not None:
        operator = node.child_by_field_name("operator")
        weight += int(operator is not None and _text(operator) in spellings)
    tokens = rules.token_operators.get(node.type)
    if tokens is not None:
        weight += sum(1 for child in node.children if not child.is_named and child.type in tokens)
    return weight


def _leaf_sloc_lines(_source: str, root: Node) -> frozenset[int]:
    lines: set[int] = set()
    stack = [root]
    while stack:
        node = stack.pop()
        if node.type in COMMENT_NODE_TYPES:
            continue
        if node.child_count:
            stack.extend(node.children)
        elif _text(node).strip():
            lines.add(_first_line(node))
    return frozenset(lines)


_PYTHON_IGNORED_TOKENS = frozenset(
    {
        tokenize.COMMENT,
        tokenize.DEDENT,
        tokenize.ENDMARKER,
        tokenize.INDENT,
        tokenize.NEWLINE,
        tokenize.NL,
    }
)


def _python_sloc_lines(source: str, root: Node) -> frozenset[int]:
    # scb-check v0.1.3 feeds the tokenizer `str.splitlines()`, so U+2028 and
    # form feeds move token lines. Parity keeps that.
    readline = iter(source.splitlines(keepends=True)).__next__
    lines = {
        token.start[0]
        for token in tokenize.generate_tokens(readline)
        if token.type not in _PYTHON_IGNORED_TOKENS
    }
    text_lines = source.splitlines()
    for child in _statement_strings(root):
        if _is_docstring(child, text_lines):
            lines.difference_update(range(_first_line(child), _last_line(child) + 1))
    return frozenset(lines)


_STATEMENT_STRING = "statement_string"


@cache
def _statement_string_query() -> Query:
    patterns = " ".join(
        f"({parent} (string) @{_STATEMENT_STRING})" for parent in sorted(PYTHON_STATEMENT_PARENTS)
    )
    query: Query = Query(grammars.get_language("python"), patterns)
    return query


def _statement_strings(root: Node) -> list[Node]:
    # The query finds the same `string` children of a `block` or `module` that a
    # walk over every node finds, in the C matcher instead of the interpreter.
    cursor = QueryCursor(_statement_string_query())
    found = cursor.captures(root).get(_STATEMENT_STRING, [])
    if cursor.did_exceed_match_limit:
        message = "the Python docstring query exceeded its match limit, so SLOC would be wrong"
        raise RuntimeError(message)
    return found


def _is_docstring(node: Node, text_lines: list[str]) -> bool:
    if node.type != "string":
        return False
    prefix = string_prefix(_text(node))
    return "b" not in prefix and "f" not in prefix and _owns_its_lines(node, text_lines)


def _owns_its_lines(literal: Node, text_lines: list[str]) -> bool:
    # Tree-sitter byte columns slice `str` lines, as scb-check does (loc.py:82),
    # so non-ASCII text before a column shifts the test. Parity keeps that.
    start_row, start_col = literal.start_point
    end_row, end_col = literal.end_point
    start_line = text_lines[start_row] if start_row < len(text_lines) else ""
    end_line = text_lines[end_row] if end_row < len(text_lines) else ""
    return not start_line[:start_col].strip() and not end_line[end_col:].strip()


_C_DECLARATOR_NAMES = frozenset({"identifier"})
_CPP_DECLARATOR_NAMES = frozenset(
    {
        "identifier",
        "field_identifier",
        "qualified_identifier",
        "destructor_name",
        "operator_name",
        "operator_cast",
        "template_function",
    }
)
_DECLARATOR_WRAPPERS = frozenset({"parenthesized_declarator", "attributed_declarator"})


def _declarator_name(node: Node, names: frozenset[str]) -> Node | None:
    current = node.child_by_field_name("declarator")
    while current is not None and current.type not in names:
        following = current.child_by_field_name("declarator")
        if following is None and current.type in _DECLARATOR_WRAPPERS:
            following = next(iter(current.named_children), None)
        current = following
    return current


def _c_name(node: Node) -> Node | None:
    if node.type == "function_definition":
        return _declarator_name(node, _C_DECLARATOR_NAMES)
    return node.child_by_field_name("name")


def _cpp_name(node: Node) -> Node | None:
    if node.type == "function_definition":
        return _declarator_name(node, _CPP_DECLARATOR_NAMES)
    return node.child_by_field_name("name")


def _go_receiver_type(node: Node) -> Node | None:
    receiver = node.child_by_field_name("receiver")
    return None if receiver is None else _first_of_type(receiver, "type_identifier")


def _rust_name(node: Node) -> Node | None:
    if node.type != "impl_item":
        return node.child_by_field_name("name")
    implemented = node.child_by_field_name("type")
    return None if implemented is None else _first_of_type(implemented, "type_identifier")


_JAVASCRIPT_BINDINGS = MappingProxyType(
    {"variable_declarator": "name", "field_definition": "property", "pair": "key"}
)
_TYPESCRIPT_BINDINGS = MappingProxyType(
    {"variable_declarator": "name", "public_field_definition": "name", "pair": "key"}
)
_ECMASCRIPT_BINDING_NAMES = frozenset(
    {"identifier", "property_identifier", "private_property_identifier", "string"}
)


def _ecmascript_binding(callable_node: Node, bindings: Mapping[str, str]) -> Node | None:
    parent = callable_node.parent
    if parent is None or parent.type not in bindings:
        return None
    value = parent.child_by_field_name("value")
    if value is None or value.id != callable_node.id:
        return None
    name = parent.child_by_field_name(bindings[parent.type])
    return name if name is not None and name.type in _ECMASCRIPT_BINDING_NAMES else None


def _javascript_binding(node: Node) -> Node | None:
    return _ecmascript_binding(node, _JAVASCRIPT_BINDINGS)


def _typescript_binding(node: Node) -> Node | None:
    return _ecmascript_binding(node, _TYPESCRIPT_BINDINGS)


_LUA_BINDING_NAMES = frozenset({"identifier", "dot_index_expression"})


def _lua_binding(node: Node) -> Node | None:
    statement = _lua_assignment_of(node)
    return None if statement is None else _lua_single_target(statement)


def _lua_assignment_of(node: Node) -> Node | None:
    values = node.parent
    if values is None or values.type != "expression_list":
        return None
    first = next(iter(values.children_by_field_name("value")), None)
    statement = values.parent
    if first is None or first.id != node.id or statement is None:
        return None
    return statement if statement.type == "assignment_statement" else None


def _lua_single_target(statement: Node) -> Node | None:
    targets = next((child for child in statement.children if child.type == "variable_list"), None)
    names = [] if targets is None else targets.children_by_field_name("name")
    if len(names) != 1 or names[0].type not in _LUA_BINDING_NAMES:
        return None
    return names[0]


def _has_named_child(node: Node) -> bool:
    return node.named_child_count > 0


def _is_not_default_case(node: Node) -> bool:
    return not any(child.type == "default" for child in node.children if not child.is_named)


def _is_counted_rust_arm(node: Node) -> bool:
    pattern = node.child_by_field_name("pattern")
    return pattern is None or [child.type for child in pattern.children] != ["_"]


def _has_condition(node: Node) -> bool:
    return node.child_by_field_name("condition") is not None


def _is_counted_ruby_arm(node: Node) -> bool:
    if node.child_by_field_name("guard") is not None:
        return True
    pattern = node.child_by_field_name("pattern")
    return pattern is None or not (pattern.type == "identifier" and _text(pattern) == "_")


def _is_counted_case_item(node: Node) -> bool:
    values = node.children_by_field_name("value")
    return not (len(values) == 1 and _text(values[0]) == "*")


_ECMASCRIPT_UNITS = UnitRules(
    units=frozenset(
        {"function_declaration", "generator_function_declaration", "method_definition"}
    ),
    scopes=frozenset({"class_declaration", "class"}),
    bound=frozenset({"arrow_function", "function_expression", "generator_function"}),
    reads=frozenset(_JAVASCRIPT_BINDINGS) | _ECMASCRIPT_BINDING_NAMES,
    binding_of=_javascript_binding,
)
_TYPESCRIPT_UNITS = UnitRules(
    units=_ECMASCRIPT_UNITS.units,
    scopes=frozenset({"class_declaration", "abstract_class_declaration", "class"}),
    bound=_ECMASCRIPT_UNITS.bound,
    reads=frozenset(_TYPESCRIPT_BINDINGS) | _ECMASCRIPT_BINDING_NAMES,
    binding_of=_typescript_binding,
)
_ECMASCRIPT_DECISIONS = DecisionRules(
    points=frozenset(
        {
            "if_statement",
            "for_statement",
            "for_in_statement",
            "while_statement",
            "do_statement",
            "catch_clause",
            "ternary_expression",
            "switch_case",
        }
    ),
    field_operators=MappingProxyType({"binary_expression": frozenset({"&&", "||", "??"})}),
)
_C_POINTS = frozenset(
    {"if_statement", "for_statement", "while_statement", "do_statement", "conditional_expression"}
)
_C_CASES = MappingProxyType({"case_statement": _is_not_default_case})
_AND_OR = frozenset({"&&", "||"})

RULES: Mapping[str, LanguageRules] = MappingProxyType(
    {
        "bash": LanguageRules(
            units=UnitRules(units=frozenset({"function_definition"})),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if_statement",
                        "elif_clause",
                        "for_statement",
                        "c_style_for_statement",
                        "while_statement",
                        "ternary_expression",
                    }
                ),
                conditional=MappingProxyType({"case_item": _is_counted_case_item}),
                # `-a` and `-o` count only as a binary operator: `[ -a file ]`
                # is a unary file test with the same spelling.
                field_operators=MappingProxyType(
                    {"binary_expression": frozenset({"&&", "||", "-a", "-o"})}
                ),
                token_operators=MappingProxyType({"list": _AND_OR}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "c": LanguageRules(
            units=UnitRules(
                units=frozenset({"function_definition"}),
                reads=frozenset({"function_declarator"})
                | _C_DECLARATOR_NAMES
                | _DECLARATOR_WRAPPERS,
                name_of=_c_name,
            ),
            decisions=DecisionRules(
                points=_C_POINTS,
                conditional=_C_CASES,
                field_operators=MappingProxyType({"binary_expression": _AND_OR}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "cpp": LanguageRules(
            units=UnitRules(
                units=frozenset({"function_definition"}),
                scopes=frozenset(
                    {
                        "class_specifier",
                        "struct_specifier",
                        "union_specifier",
                        "namespace_definition",
                    }
                ),
                reads=frozenset({"function_declarator"})
                | _CPP_DECLARATOR_NAMES
                | _DECLARATOR_WRAPPERS,
                name_of=_cpp_name,
            ),
            decisions=DecisionRules(
                points=_C_POINTS | {"for_range_loop", "catch_clause"},
                conditional=_C_CASES,
                field_operators=MappingProxyType(
                    {"binary_expression": frozenset({"&&", "||", "and", "or"})}
                ),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "go": LanguageRules(
            units=UnitRules(
                units=frozenset({"function_declaration", "method_declaration"}),
                reads=frozenset({"type_identifier"}),
                owner_of=_go_receiver_type,
            ),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if_statement",
                        "for_statement",
                        "expression_case",
                        "type_case",
                        "communication_case",
                    }
                ),
                field_operators=MappingProxyType({"binary_expression": _AND_OR}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "java": LanguageRules(
            units=UnitRules(
                units=frozenset(
                    {
                        "method_declaration",
                        "constructor_declaration",
                        "compact_constructor_declaration",
                    }
                ),
                scopes=frozenset(
                    {
                        "class_declaration",
                        "interface_declaration",
                        "enum_declaration",
                        "record_declaration",
                    }
                ),
            ),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if_statement",
                        "for_statement",
                        "enhanced_for_statement",
                        "while_statement",
                        "do_statement",
                        "catch_clause",
                        "ternary_expression",
                        "guard",
                        "assert_statement",
                    }
                ),
                conditional=MappingProxyType({"switch_label": _has_named_child}),
                field_operators=MappingProxyType({"binary_expression": _AND_OR}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "javascript": LanguageRules(
            units=_ECMASCRIPT_UNITS,
            decisions=_ECMASCRIPT_DECISIONS,
            sloc_lines=_leaf_sloc_lines,
        ),
        "lua": LanguageRules(
            units=UnitRules(
                units=frozenset({"function_declaration"}),
                bound=frozenset({"function_definition"}),
                reads=frozenset({"expression_list", "assignment_statement", "variable_list"})
                | _LUA_BINDING_NAMES,
                binding_of=_lua_binding,
            ),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if_statement",
                        "elseif_statement",
                        "for_statement",
                        "while_statement",
                        "repeat_statement",
                    }
                ),
                token_operators=MappingProxyType({"binary_expression": frozenset({"and", "or"})}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "python": LanguageRules(
            # scb-check's walker enters only these, so a function inside a
            # module-level `if` or `try` is not a unit.
            units=UnitRules(
                units=frozenset({"function_definition"}),
                scopes=frozenset({"class_definition"}),
                descend=frozenset({"class_definition", "decorated_definition", "block"}),
            ),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if_statement",
                        "elif_clause",
                        "for_statement",
                        "while_statement",
                        "except_clause",
                        "assert_statement",
                        "list_comprehension",
                        "set_comprehension",
                        "dictionary_comprehension",
                        "generator_expression",
                        "boolean_operator",
                        "conditional_expression",
                        "if_clause",
                    }
                )
            ),
            sloc_lines=_python_sloc_lines,
        ),
        "ruby": LanguageRules(
            units=UnitRules(
                units=frozenset({"method", "singleton_method"}),
                scopes=frozenset({"class", "module"}),
            ),
            decisions=DecisionRules(
                points=frozenset(
                    {
                        "if",
                        "elsif",
                        "unless",
                        "if_modifier",
                        "unless_modifier",
                        "while",
                        "until",
                        "for",
                        "while_modifier",
                        "until_modifier",
                        "rescue",
                        "rescue_modifier",
                        "conditional",
                        "when",
                        "if_guard",
                        "unless_guard",
                    }
                ),
                conditional=MappingProxyType({"in_clause": _is_counted_ruby_arm}),
                field_operators=MappingProxyType({"binary": frozenset({"&&", "||", "and", "or"})}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "rust": LanguageRules(
            units=UnitRules(
                units=frozenset({"function_item"}),
                scopes=frozenset({"impl_item", "trait_item", "mod_item"}),
                reads=frozenset({"type_identifier"}),
                name_of=_rust_name,
            ),
            decisions=DecisionRules(
                points=frozenset({"if_expression", "for_expression", "while_expression"}),
                conditional=MappingProxyType(
                    {"match_arm": _is_counted_rust_arm, "match_pattern": _has_condition}
                ),
                field_operators=MappingProxyType({"binary_expression": _AND_OR}),
                token_operators=MappingProxyType({"let_chain": frozenset({"&&"})}),
            ),
            sloc_lines=_leaf_sloc_lines,
        ),
        "tsx": LanguageRules(
            units=_TYPESCRIPT_UNITS,
            decisions=_ECMASCRIPT_DECISIONS,
            sloc_lines=_leaf_sloc_lines,
        ),
        "typescript": LanguageRules(
            units=_TYPESCRIPT_UNITS,
            decisions=_ECMASCRIPT_DECISIONS,
            sloc_lines=_leaf_sloc_lines,
        ),
    }
)
