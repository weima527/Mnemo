# Test Fixtures

This directory contains small synthetic repositories used for integration tests.

## Structure

Each subdirectory is a standalone git repository with a known state:

```
tests/fixtures/
├── basic_rust/        # A minimal Rust project with a few functions
├── renamed_symbol/    # A repo with a renamed function (for incremental test)
├── import_update/     # A repo with an import change
├── pr_overlay/        # Two branches simulating a PR
└── multi_file/        # Multiple files with cross-file dependencies
```

## Usage

Tests should create a temporary copy of a fixture, run the tool under test,
and assert deterministic output. Do not mutate the originals.
