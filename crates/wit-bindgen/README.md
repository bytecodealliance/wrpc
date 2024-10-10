<div align="center">
  <h1><code>wit-bindgen-wrpc</code></h1>

  <p>
    <strong>wRPC native application Rust language bindings generator for
    <a href="https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md">WIT</a>
    and the
    <a href="https://github.com/WebAssembly/component-model">Component Model</a>
    </strong>
  </p>

  <p>
    <a href="https://docs.rs/wit-bindgen-wrpc"><img src="https://docs.rs/wit-bindgen-wrpc/badge.svg" alt="Documentation Status" /></a>
  </p>
</div>

# About

This crate provides a macro, [`generate!`], to automatically generate Rust
wRPC bindings for a [WIT] [world]. For more information about this crate see the
[online documentation] which includes some examples and longer form reference
documentation as well.

This crate is developed as a portion of the [`wRPC` repository] which
also contains a CLI and the RPC framework itself

[`generate!`]: https://docs.rs/wit-bindgen-wrpc/latest/wit_bindgen_wrpc/macro.generate.html
[WIT]: https://component-model.bytecodealliance.org/design/wit.html
[world]: https://component-model.bytecodealliance.org/design/worlds.html
[online documentation]: https://docs.rs/wit-bindgen-wrpc
[`wRPC` repository]: https://github.com/bytecodealliance/wrpc

# License

This project is licensed under the Apache 2.0 license with the LLVM exception.
See [LICENSE](../../LICENSE) for more details.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be licensed as above, without any additional terms or conditions.
