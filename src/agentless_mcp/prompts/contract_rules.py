"""The ``contract_drift`` rules, as one typed record per rule and language."""

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from agentless_mcp.prompts.loader import PromptDataError, resource_text

FILENAME = "contract_rules.json"
SEVERITIES = ("warning", "advisory")

_TOP_KEYS = frozenset({"rules"})
_RULE_KEYS = frozenset({"id", "languages", "severity", "query", "message"})


@dataclass(frozen=True)
class ContractRule:
    """One contract rule for one language: its id, severity, query, and message."""

    id: str
    language: str
    severity: str
    query: str
    message: str


def load_contract_rules() -> tuple[ContractRule, ...]:
    """Read the packaged rule file and return every rule, one record per language."""
    return parse_contract_rules(FILENAME, resource_text(FILENAME))


def parse_contract_rules(filename: str, text: str) -> tuple[ContractRule, ...]:
    """Decode one rule file into typed records, or raise PromptDataError."""
    try:
        document: Any = json.loads(text)
    except json.JSONDecodeError as exc:
        message = f"rule file {filename!r} is not valid JSON: {exc}"
        raise PromptDataError(message) from exc

    _require_keys(filename, "the top level", document, _TOP_KEYS)
    entries = document["rules"]
    if not isinstance(entries, list) or not entries:
        message = f"rule file {filename!r} key 'rules' must be a non-empty list"
        raise PromptDataError(message)

    rules = [rule for index, entry in enumerate(entries) for rule in _rules(filename, index, entry)]
    seen: set[tuple[str, str]] = set()
    for rule in rules:
        if (rule.id, rule.language) in seen:
            message = f"rule file {filename!r} defines rule {rule.id!r} twice for {rule.language}"
            raise PromptDataError(message)
        seen.add((rule.id, rule.language))
    return tuple(rules)


def _rules(filename: str, index: int, entry: Any) -> list[ContractRule]:
    where = f"rule {index}"
    _require_keys(filename, where, entry, _RULE_KEYS)
    fields = {key: _text(filename, where, entry, key) for key in ("id", "query", "message")}
    severity = entry["severity"]
    if severity not in SEVERITIES:
        message = f"rule file {filename!r} {where} severity must be one of {', '.join(SEVERITIES)}"
        raise PromptDataError(message)
    return [
        ContractRule(language=language, severity=severity, **fields)
        for language in _languages(filename, where, entry["languages"])
    ]


def _require_keys(filename: str, where: str, value: Any, expected: frozenset[str]) -> None:
    if not isinstance(value, dict):
        message = f"rule file {filename!r}: {where} must be a JSON object"
        raise PromptDataError(message)
    missing = sorted(expected - set(value))
    unknown = sorted(set(value) - expected)
    if missing or unknown:
        message = (
            f"rule file {filename!r}: {where} does not match the keys the code consumes: "
            f"missing {missing}, unknown {unknown}"
        )
        raise PromptDataError(message)


def _text(filename: str, where: str, entry: Mapping[str, Any], key: str) -> str:
    value = entry[key]
    if not isinstance(value, str) or not value.strip():
        message = f"rule file {filename!r} {where} key {key!r} must be a non-empty string"
        raise PromptDataError(message)
    return value


def _languages(filename: str, where: str, value: Any) -> list[str]:
    if (
        not isinstance(value, list)
        or not value
        or not all(isinstance(language, str) and language.strip() for language in value)
        or len(set(value)) != len(value)
    ):
        message = (
            f"rule file {filename!r} {where} key 'languages' must be a non-empty list "
            "of distinct non-empty strings"
        )
        raise PromptDataError(message)
    return list(value)
