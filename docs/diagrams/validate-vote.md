# Patch validation and the vote

The write side, CLI only: several candidate fixes go in, ranked evidence
comes out. The two red outcomes exist so that no verdict is ever computed
against a baseline that could not have told the truth.

```mermaid
flowchart TD
    C(["candidates/ directory\none SEARCH/REPLACE file per fix,\nnamed in sampling order"])

    C --> B["Baseline: run --test-cmd on unpatched HEAD\n(--repeat-baseline N runs it N times)"]

    B -->|"red"| UV["UNVERIFIED\nnothing is evaluated: a red baseline\ncannot tell your regression from its own"]
    B -->|"answers differ\nacross N runs"| FLAKY["UNVERIFIED (flaky)\nnames how many runs disagreed"]
    B -->|"green"| RP{"--repro-cmd given?"}

    RP -->|"passes on\nunpatched HEAD"| DNR["does_not_reproduce\nthe repro test pins nothing;\nrepro rung removed, regression still runs"]
    RP -->|"fails on HEAD\n(good: it reproduces)"| PLAN
    RP -->|"none given"| PLAN
    DNR --> PLAN

    PLAN["Normalize every candidate against HEAD;\nbyte-identical resulting file states form\none execution group, while every id and vote remains"]
    PLAN --> RUN["Each execution-group representative:\nfresh throwaway worktree at HEAD,\napply -> regression, hard timeout,\nhang = FAILURE, checkout never touched"]

    RUN -->|"regression fails"| SKIP["reproduction = not_evaluated\nno reproduction command is spent"]
    RUN -->|"regression passes and\nrepro is valid"| REPRO["run reproduction command"]
    RUN -->|"no valid repro"| J
    SKIP --> J
    REPRO --> J

    J["verdicts.jsonl\napply status, regression, reproduction,\nAST-equivalence key, execution_group, executed_as"]

    J --> V["vote: strongest non-empty tier wins"]
    V --> T1["regression + reproduction\nfixed the bug, broke nothing"]
    V --> T2["regression only\nbroke nothing; nothing proved a fix"]
    V --> T3["applied only\n= the report that nothing worked"]

    T1 --> CL["Cluster survivors by AST-equivalence:\ntwo spellings of one fix = two votes\nfor one change. Rank by cluster size,\nties by first appearance."]
    T2 --> CL
    CL --> REP(["representative patch\n-> patch apply"])
```
