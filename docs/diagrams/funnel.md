# The localization funnel

The tool from the agent's point of view: two calls answer most questions,
and every arrow downward spends fewer tokens on more relevant code. The
numbers on the nodes are the research constraints the defaults encode.

```mermaid
flowchart TD
    ISSUE(["An issue: 'search sometimes returns nothing'"])

    ISSUE --> M["Call 1: repo_map\nfocus = files/symbols the issue mentions\nPageRank flows outward from the seeds\n-> ranked skeleton, capped at 10 files"]

    M --> READ{{"Read the receipt first:\nright repo? right HEAD? cache fresh?"}}

    READ --> E["Call 2: expand_symbols\nonly the stable ids the skeleton implicated\n-> full bodies, line-numbered"]

    E --> ENOUGH{"Is a body enough?"}
    ENOUGH -- "usually yes" --> FIX["Reason and write the fix\n(~4.5k-8.7k tokens gathered total:\nthe ~6x compression band that beats\nboth full files and over-compression)"]
    ENOUGH -- "rarely" --> S["read_slice\nexact line ranges, sticky-scroll headers\n(line level is the last resort:\nit measurably degrades repair)"]
    S --> FIX

    M -.->|"who calls this?\nblast radius"| R["find_referencing_symbols\nfan-in grouped by file"]
    M -.->|"does a utility\nalready exist?"| SC["find_referencing_symbols\nshared_callers: true"]
    R -.-> E
    SC -.-> E

    FIX --> V["Several candidate fixes?\n-> validate + vote (CLI)\nsee validate-vote.md"]
```
