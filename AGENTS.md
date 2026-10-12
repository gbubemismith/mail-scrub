# Agent guidelines

## Standards

- After every code change run `scripts/check.sh` (fmt, clippy with warnings as errors, full tests) and fix what it reports before calling the work done. Use `--fast` to skip tests while iterating. Hooks and CI run the same script.
- No `unwrap`, `expect` or `panic!` in non-test code; clippy denies them. Return errors instead: `thiserror` types in the library, `anyhow` only in `cli.rs`.
- Clippy pedantic is on. Fix warnings rather than adding `#[allow]`; if an allow is truly warranted, scope it tightly and say why.
- Keep `main.rs` thin. Logic lives in the library.
- Match the surrounding code's naming, comment density and idiom.

## Testing

- Test behaviour through public interfaces: assert on labels and filters in the fake client, recorded Runs, and what the binary prints. Never assert on channel wiring, stage internals, or SQL.
- The primary seam is a whole Run against `FakeClient` (`tests/run.rs`). Copy that pattern for new features. Classification is tested as a pure function with table-driven cases.
- Write the failing test first, then only enough code to pass it, one slice at a time.
- Expected values come from the spec or a literal, never recomputed the way the code computes them.
- Tests may use `unwrap` and `expect`. When a contract changes, update the assertion rather than preserving old behaviour.
