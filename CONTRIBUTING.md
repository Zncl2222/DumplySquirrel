# Contributing

## GitHub Flow

This project uses GitHub Flow:

1. Keep `main` deployable.
2. Create a short-lived branch from `main` for every change.
3. Open a pull request early so CI can run and reviewers can see the direction.
4. Keep the branch up to date with `main` before merging when needed.
5. Merge only after CI passes and the pull request is reviewed.
6. Delete the branch after merge.

Recommended branch names:

- `feature/<short-description>`
- `fix/<short-description>`
- `chore/<short-description>`

## Local Checks

Install pre-commit once:

```bash
pip install pre-commit
pre-commit install
```

Run all pre-commit checks manually:

```bash
pre-commit run --all-files
```

Backend checks:

```bash
cd backend
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Frontend checks:

```bash
cd frontend
npm ci
npm audit --audit-level=high
npm test
npm run build
```

CI runs these checks on pull requests and on pushes to `main`.
