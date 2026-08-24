"""Keep the fixture repositories out of pytest's collection.

``repo_py_tests`` holds files named ``tests/test_*.py`` because that is what
the companion section under test exists to find, and pytest reads the same
convention as an instruction to import and run them. They import ``pricing``
and ``cli`` -- modules of the fixture repository, not of this project -- so
collection fails at import before any real test runs.

Ignoring the whole directory rather than that one repository: every path
under here is input to a parser, and none of it is ever meant to execute.
"""

collect_ignore_glob = ["*"]
