# Contributing to wRPC

Thank you for taking the time to read this document and considering contributing to wRPC!

## Reporting bugs

wRPC project follows rigorous testing procedure and we try our best to produce fail-safe, resilient code, however - bugs happen and if you encounter one, please report it by opening an issue explaining it.

We would appreciate if you would describe your environment as detailed as possible (OS, wRPC version, programming language(s), transport protocol and service version) and provide reproduction steps for the issue. 

If possible and/or desired, the best way to report a bug would be contributing a test case reproducing it.

## Proposing new features

Do you want wRPC to do something it does not already? Great! Please file an issue describing the feature and the intended use case(s). If you can, try to propose the design and UX of the feature.

## Submitting changes

If you are interested in working on a particular issue, please let us know by commenting on the issue you would like to work on.
Project maintainers are trying to assign "help wanted" to issues, that would greatly benefit from contributions and "good first issue" as recommendations for issues to work on - this is by no means a prescription, and if you find an issue you would like to work on, which is not already in-progress, please comment on the issue and get hacking!

Ideally, pull requests should be preceded by an issue and possible discussion. That is not a requirement, but it certainly smoothens the review process.

### Commit guidelines

wRPC project developers try to follow [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/) specification for commit messages and we would appreciate if, in your contributions, you did as well.

At least at this point in wRPC project's lifecycle this is just a guideline and not a requirement for a pull request to get merged.

As a general rule, try to avoid unnecessary changes as much as possible and keep your commits well-scoped and atomic.

For code changes, we generally value integration-style, easy to maintain, test cases more than highly isolated and narrowly-scoped unit tests. There is no hard-and-fast rule here, but try to test behavior and intended use cases of the project as a whole, rather than implementation specifics.

If the contributed code is not already covered by an existing test case, please consider adding it in your pull request. Generally, small, obvious, pull requests can be merged without a test case, but we require a test case for a bigger change.

### Rust code

Rust code should follow [Rust Style Guide](https://github.com/rust-lang/rust/tree/5ee2dfd2bcbb66a69a46aaa342204e0dfdb70516/src/doc/style-guide/src).

The following commands must pass before a pull request can get merged:
- `cargo fmt --all`
- `cargo clippy --workspace`
- `cargo doc --workspace`
- `cargo test --workspace`

We encourage contributors to additionally run:
```
cargo clippy --fix --allow-dirty --allow-staged --all-targets --workspace -- -Wclippy::pedantic
```
on their code to automatically fix some of the lint issues in their code and point out some potential improvements. Your code is not required to comply with [`clippy::pedantic`] and, in fact, you are more than likely to find out that existing codebase fails some of the checks, but we strive to follow [`clippy::pedantic`] suggestions where applicable and always apply `--fix` improvements.

If the contributed code is not already covered by an existing test case, please consider adding it in your pull request. Generally, small, obvious, pull requests can be merged without a test case, but we require a test case for a bigger change.

### Go code

Go code should follow [Effective Go](https://go.dev/doc/effective_go) and [Go Code Review Comments](https://go.dev/wiki/CodeReviewComments).

All Go code should be formatted using `gofmt -w -s` or a stricter linter.

Go code is tested as part of the `cargo` test suite, meaning that e.g. `cargo test go` can (and should) be used to test your Go code.

[`clippy::pedantic`]: https://doc.rust-lang.org/stable/clippy/usage.html#clippypedantic
