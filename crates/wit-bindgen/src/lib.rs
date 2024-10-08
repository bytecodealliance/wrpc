//! Bindings generation support for wRPC using Rust with the Component Model.
//!
//! This crate is a bindings generator for [WIT] and the [Component Model].
//! Users are likely interested in the [`generate!`] macro which actually
//! generates bindings. Otherwise this crate provides any runtime support
//! necessary for the macro-generated code.
//!
//! [WIT]: https://component-model.bytecodealliance.org/design/wit.html
//! [Component Model]: https://component-model.bytecodealliance.org/

/// Generate bindings for an input WIT document.
///
/// This macro is the bread-and-butter of the `wit-bindgen-wrpc` crate. The macro
/// here will parse [WIT] as input and generate Rust bindings to work with the
/// `world` that's specified in the [WIT]. For a primer on WIT see [this
/// documentation][WIT] and for a primer on worlds see [here][worlds].
///
/// [WIT]: https://component-model.bytecodealliance.org/design/wit.html
/// [worlds]: https://component-model.bytecodealliance.org/design/worlds.html
///
/// This macro takes as input a [WIT package] as well as a [`world`][worlds]
/// within that package. It will then generate a Rust function for all `import`s
/// into the world. If there are any `export`s then a Rust `trait` will be
/// generated for you to implement. The macro additionally takes a number of
/// configuration parameters documented below as well.
///
/// Basic invocation of the macro can look like:
///
/// ```
/// use wit_bindgen_wrpc::generate;
/// # macro_rules! generate { ($($t:tt)*) => () }
///
/// generate!();
/// ```
///
/// This will parse a WIT package in the `wit` folder adjacent to your project's
/// `Cargo.toml` file. Within this WIT package there must be precisely one
/// `world` and that world will be the one that has bindings generated for it.
/// All other options remain at their default values (more on this below).
///
/// If your WIT package has more than one `world`, or if you want to select a
/// world from the dependencies, you can specify a world explicitly:
///
/// ```
/// use wit_bindgen_wrpc::generate;
/// # macro_rules! generate { ($($t:tt)*) => () }
///
/// generate!("my-world");
/// generate!("wasi:cli/imports");
/// ```
///
/// This form of the macro takes a single string as an argument which is a
/// "world specifier" to select which world is being generated. As a single
/// string, such as `"my-world"`, this selects the world named `my-world` in the
/// package being parsed in the `wit` folder. The longer form specification
/// `"wasi:cli/imports"` indicates that the `wasi:cli` package, located in the
/// `wit/deps` folder, will have a world named `imports` and those bindings will
/// be generated.
///
/// If your WIT package is located in a different directory than one called
/// `wit` then it can be specified with the `in` keyword:
///
/// ```
/// use wit_bindgen_wrpc::generate;
/// # macro_rules! generate { ($($t:tt)*) => () }
///
/// generate!(in "./my/other/path/to/wit");
/// generate!("a-world" in "../path/to/wit");
/// ```
///
/// The full-form of the macro, however, takes a braced structure which is a
/// "bag of options":
///
/// ```
/// use wit_bindgen_wrpc::generate;
/// # macro_rules! generate { ($($t:tt)*) => () }
///
/// generate!({
///     world: "my-world",
///     path: "../path/to/wit",
///     // ...
/// });
/// ```
///
/// For documentation on each option, see below.
///
/// ## Exploring generated bindings
///
/// Once bindings have been generated they can be explored via a number of means
/// to see what was generated:
///
/// * Using `cargo doc` should render all of the generated bindings in addition
///   to the original comments in the WIT format itself.
/// * If your IDE supports `rust-analyzer` code completion should be available
///   to explore and see types.
/// * The `wit-bindgen-wrpc` CLI tool, packaged as `wit-bindgen-wrpc-cli` on crates.io,
///   can be executed the same as the `generate!` macro and the output can be
///   read.
/// * If you're seeing an error, `WIT_BINDGEN_DEBUG=1` can help debug what's
///   happening (more on this below) by emitting macro output to a file.
/// * This documentation can be consulted for various constructs as well.
///
/// Currently browsing generated code may have road bumps on the way. If you run
/// into issues or have idea of how to improve the situation please [file an
/// issue].
///
/// [file an issue]: https://github.com/bytecodealliance/wrpc/issues/new
///
/// ## Namespacing
///
/// In WIT, worlds can import and export `interface`s, functions, and types. Each
/// `interface` can either be "anonymous" and only named within the context of a
/// `world` or it can have a "package ID" associated with it. Names in Rust take
/// into account all the names associated with a WIT `interface`. For example
/// the package ID `foo:bar/baz` would create a `mod foo` which contains a `mod
/// bar` which contains a `mod baz`.
///
/// WIT imports and exports are additionally separated into their own
/// namespaces. Imports are generated at the level of the `generate!` macro
/// where exports are generated under an `exports` namespace.
///
/// ## Imports
///
/// Imports into a `world` can be types, resources, functions, and interfaces.
/// Each of these is bound as a Rust type, function, or module. The intent is
/// that the WIT interfaces map to what is roughly idiomatic Rust for the given
/// interface.
///
/// ### Imports: Top-level functions and types
///
/// Imports at the top-level of a world are generated directly where the
/// `generate!` macro is invoked.
///
/// ```
/// mod bindings {
///     use wit_bindgen_wrpc::generate;
///
///     generate!({
///         inline: r"
///             package a:b;
///
///             world the-world {
///                 record fahrenheit {
///                     degrees: f32,
///                 }
///
///                 import what-temperature-is-it: func() -> fahrenheit;
///
///                 record celsius {
///                     degrees: f32,
///                 }
///
///                 import convert-to-celsius: func(a: fahrenheit) -> celsius;
///             }
///         ",
///     });
/// }
///
/// use bindings::Celsius;
///
/// async fn test(wrpc: &impl wrpc_transport::Invoke<Context = ()>) -> anyhow::Result<()> {
///     let current_temp = bindings::what_temperature_is_it(wrpc, ()).await?;
///     println!("current temp in fahrenheit is {}", current_temp.degrees);
///     let in_celsius: Celsius = bindings::convert_to_celsius(wrpc, (), &current_temp).await?;
///     println!("current temp in celsius is {}", in_celsius.degrees);
///     Ok(())
/// }
/// ```
///
/// ### Imports: Interfaces
///
/// Interfaces are placed into submodules where the `generate!` macro is
/// invoked and are namespaced based on their identifiers.
///
/// ```
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     inline: r"
///         package my:test;
///
///         interface logging {
///             enum level {
///                 debug,
///                 info,
///                 error,
///             }
///             log: func(level: level, msg: string);
///         }
///
///         world the-world {
///             import logging;
///             import global-logger: interface {
///                 use logging.{level};
///
///                 set-current-level: func(level: level);
///                 get-current-level: func() -> level;
///             }
///         }
///     ",
/// });
///
/// // `my` and `test` are from `package my:test;` and `logging` is for the
/// // interface name.
/// use my::test::logging::Level;
///
/// async fn test(wrpc: &impl wrpc_transport::Invoke<Context = ()>) -> anyhow::Result<()> {
///     let current_level = global_logger::get_current_level(wrpc, ()).await?;
///     println!("current logging level is {current_level:?}");
///     global_logger::set_current_level(wrpc, (), Level::Error).await?;
///
///     my::test::logging::log(wrpc, (), Level::Info, "Hello there!").await?;
///     Ok(())
/// }
/// #
/// # fn main() {}
/// ```
///
/// ### Imports: Resources
///
/// Imported resources generate a type named after the name of the resource.
/// This type is then used both for borrows as `&T` as well as via ownership as
/// `T`. Resource methods are bound as methods on the type `T`.
///
/// ```
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     inline: r#"
///         package my:test;
///
///         interface logger {
///             enum level {
///                 debug,
///                 info,
///                 error,
///             }
///
///             resource logger {
///                 constructor(destination: string);
///                 log: func(level: level, msg: string);
///             }
///         }
///
///         // Note that while this world does not textually import the above
///         // `logger` interface it is a transitive dependency via the `use`
///         // statement so the "elaborated world" imports the logger.
///         world the-world {
///             use logger.{logger};
///
///             import get-global-logger: func() -> logger;
///         }
///     "#,
/// });
///
/// use my::test::logger::{self, Level};
///
/// async fn test(wrpc: &impl wrpc_transport::Invoke<Context = ()>) -> anyhow::Result<()> {
///     let logger = get_global_logger(wrpc, ()).await?;
///     Logger::log(wrpc, (), &logger.as_borrow(), Level::Debug, "This is a global message");
///
///     let logger2 = Logger::new(wrpc, (), "/tmp/other.log").await?;
///     Logger::log(wrpc, (), &logger2.as_borrow(), Level::Info, "This is not a global message").await?;
///     Ok(())
/// }
/// #
/// # fn main() {}
/// ```
///
/// Note in the above example the lack of import of `Logger`. The `use`
/// statement imported the `Logger` type, an alias of it, from the `logger`
/// interface into `the-world`. This generated a Rust `type` alias so `Logger`
/// was available at the top-level.
///
/// ## Exports: Basic Usage
///
/// A WIT world can not only `import` functionality but can additionally
/// `export` functionality as well. An `export` represents a contract that the
/// Rust program must implement to be able to work correctly. The `generate!`
/// macro's goal is to take care of all the low-level and ABI details for you,
/// so the end result is that `generate!`, for exports, will generate Rust
/// `trait`s that you must implement.
///
/// A minimal example of this is:
///
/// ```
/// use futures::stream::TryStreamExt as _;
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     inline: r#"
///         package my:test;
///
///         world my-world {
///             export hello: func();
///         }
///     "#,
/// });
///
/// #[derive(Clone)]
/// struct MyComponent;
///
/// impl<Ctx: Send> Handler<Ctx> for MyComponent {
///     async fn hello(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// }
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// Here the `Handler` trait was generated by the `generate!` macro and represents
/// the functions at the top-level of `my-world`, in this case the function
/// `hello`. A custom type, here called `MyComponent`, is created and the trait
/// is implemented for that type.
///
/// Additionally a macro is generated by `generate!` (macros generating macros)
/// called `serve`. The `serve` function is given a component that implements
/// the export `trait`s and then it will itself generate all necessary
/// `#[no_mangle]` functions to implement the ABI required.
///
/// ## Exports: Multiple Interfaces
///
/// Each `interface` in WIT will generate a `trait` that must be implemented in
/// addition to the top-level `trait` for the world. All traits are named
/// `Handler` here and are namespaced appropriately in modules:
///
/// ```
/// use futures::stream::TryStreamExt as _;
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     inline: r#"
///         package my:test;
///
///         interface a {
///             func-in-a: func();
///             second-func-in-a: func();
///         }
///
///         world my-world {
///             export a;
///             export b: interface {
///                 func-in-b: func();
///             }
///             export c: func();
///         }
///     "#,
/// });
///
/// #[derive(Clone)]
/// struct MyComponent;
///
/// impl<Ctx: Send> Handler<Ctx> for MyComponent {
///     async fn c(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// }
///
/// impl<Ctx: Send> exports::my::test::a::Handler<Ctx> for MyComponent {
///     async fn func_in_a(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
///     async fn second_func_in_a(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// }
///
/// impl<Ctx: Send> exports::b::Handler<Ctx> for MyComponent {
///     async fn func_in_b(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// }
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// Here note that there were three `Handler` traits generated for each of the
/// three groups: two interfaces and one `world`. Also note that traits (and
/// types) for exports are namespaced in an `exports` module.
///
/// Note that when the top-level `world` does not have any exported functions,
/// or if an interface does not have any functions, then no `trait` is
/// generated:
///
/// ```
/// use futures::stream::TryStreamExt as _;
///
/// mod bindings {
///     use wit_bindgen_wrpc::generate;
///
///     generate!({
///         inline: r#"
///             package my:test;
///
///             interface a {
///                 type my-type = u32;
///             }
///
///             world my-world {
///                 export b: interface {
///                     use a.{my-type};
///
///                     foo: func() -> my-type;
///                 }
///             }
///         "#,
///     });
/// }
///
/// #[derive(Clone)]
/// struct MyComponent;
///
/// impl<Ctx: Send> bindings::exports::b::Handler<Ctx> for MyComponent {
///     async fn foo(&self, cx: Ctx) -> anyhow::Result<u32> {
///         Ok(42)
///     }
/// }
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = bindings::serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// ## Exports: Resources
///
/// Exporting a resource is significantly different than importing a resource.
/// A component defining a resource can create new resources of that type at any
/// time, for example. Additionally resources can be "dereferenced" into their
/// underlying values within the component.
///
/// Owned resources have a custom type generated and borrowed resources are
/// generated with a type of the same name suffixed with `Borrow<'_>`, such as
/// `MyResource` and `MyResourceBorrow<'_>`.
///
/// Like `interface`s the methods and functions used with a `resource` are
/// packaged up into a `trait`.
///
/// Specifying a custom resource type is done with an associated type on the
/// corresponding trait for the resource's containing interface/world:
///
/// ```
/// use std::sync::{Arc, RwLock};
///
/// use anyhow::Context;
/// use bytes::Bytes;
/// use futures::stream::TryStreamExt as _;
/// use wrpc_transport::{ResourceBorrow, ResourceOwn};
///
/// mod bindings {
///     use wit_bindgen_wrpc::generate;
///
///     generate!({
///         inline: r#"
///             package my:test;
///
///             interface logging {
///                 enum level {
///                     debug,
///                     info,
///                     error,
///                 }
///
///                 resource logger {
///                     constructor(level: level);
///                     log: func(level: level, msg: string);
///                     level: func() -> level;
///                     set-level: func(level: level);
///                 }
///             }
///
///             world my-world {
///                 export logging;
///             }
///         "#,
///     });
/// }
///
/// use bindings::exports::my::test::logging::{Handler, HandlerLogger, Level, Logger};
///
/// #[derive(Clone, Default)]
/// struct MyComponent{
///     loggers: Arc<RwLock<Vec<MyLogger>>>,
/// }
///
/// // Note that the `logging` interface has no methods of its own but a trait
/// // is required to be implemented here to specify the type of `Logger`.
/// impl<Ctx: Send> Handler<Ctx> for MyComponent {}
///
/// struct MyLogger {
///     level: RwLock<Level>,
///     contents: RwLock<String>,
/// }
///
/// impl<Ctx: Send> HandlerLogger<Ctx> for MyComponent {
///     async fn new(&self, cx: Ctx, level: Level) -> anyhow::Result<ResourceOwn<Logger>> {
///         let mut loggers = self.loggers.write().unwrap();
///         let handle = loggers.len().to_le_bytes();
///         loggers.push(MyLogger {
///             level: RwLock::new(level),
///             contents: RwLock::new(String::new()),
///         });
///         Ok(ResourceOwn::from(Bytes::copy_from_slice(&handle)))
///     }
///
///     async fn log(&self, cx: Ctx, logger: ResourceBorrow<Logger>, level: Level, msg: String) -> anyhow::Result<()> {
///         let i = Bytes::from(logger).as_ref().try_into()?;
///         let i = usize::from_le_bytes(i);
///         let loggers = self.loggers.read().unwrap();
///         let logger = loggers.get(i).context("invalid resource handle")?;
///         if level as u32 <= *logger.level.read().unwrap() as u32 {
///             let mut contents = logger.contents.write().unwrap();
///             contents.push_str(&msg);
///             contents.push_str("\n");
///         }
///         Ok(())
///     }
///
///     async fn level(&self, cx: Ctx, logger: ResourceBorrow<Logger>) -> anyhow::Result<Level> {
///         let i = Bytes::from(logger).as_ref().try_into()?;
///         let i = usize::from_le_bytes(i);
///         let loggers = self.loggers.read().unwrap();
///         let logger = loggers.get(i).context("invalid resource handle")?;
///         let level = logger.level.read().unwrap();
///         Ok(level.clone())
///     }
///
///     async fn set_level(&self, cx: Ctx, logger: ResourceBorrow<Logger>, level: Level) -> anyhow::Result<()> {
///         let i = Bytes::from(logger).as_ref().try_into()?;
///         let i = usize::from_le_bytes(i);
///         let loggers = self.loggers.read().unwrap();
///         let logger = loggers.get(i).context("invalid resource handle")?;
///         *logger.level.write().unwrap() = level;
///         Ok(())
///     }
/// }
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = bindings::serve(wrpc, MyComponent::default()).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// It's important to note that resources in Rust do not get `&mut self` as
/// methods, but instead are required to be defined with `&self`. This requires
/// the use of interior mutability such as `RwLock` above from the
/// `std::sync` module.
///
/// ## Exports: The `serve` function
///
/// Components are created by having exported WebAssembly functions with
/// specific names, and these functions are not created when `generate!` is
/// invoked. Instead these functions are created afterwards once you've defined
/// your own type an implemented the various `trait`s for it. The `#[no_mangle]`
/// functions that will become the component are created with the generated
/// `serve` function.
///
/// Each call to `generate!` will itself generate a macro called `serve`.
/// The macro's first argument is the name of a type that implements the traits
/// generated:
///
/// ```
/// use futures::stream::TryStreamExt as _;
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     inline: r#"
///         package my:test;
///
///         world my-world {
/// #           export hello: func();
///             // ...
///         }
///     "#,
/// });
///
/// #[derive(Clone)]
/// struct MyComponent;
///
/// impl<Ctx: Send> Handler<Ctx> for MyComponent {
/// #   async fn hello(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
///     // ...
/// }
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// This argument is a Rust type which implements the `Handler` traits generated
/// by `generate!`. Note that all `Handler` traits must be implemented for the
/// type provided or an error will be generated.
///
/// This macro additionally accepts a second argument. The macro itself needs to
/// be able to find the module where the `generate!` macro itself was originally
/// invoked. Currently that can't be done automatically so a path to where
/// `generate!` was provided can also be passed to the macro. By default, the
/// argument is set to `self`:
///
/// ```
/// use futures::stream::TryStreamExt as _;
/// use wit_bindgen_wrpc::generate;
///
/// generate!({
///     // ...
/// #   inline: r#"
/// #       package my:test;
/// #
/// #       world my-world {
/// #           export hello: func();
/// #           // ...
/// #       }
/// #   "#,
/// });
/// #
/// # #[derive(Clone)]
/// # struct MyComponent;
/// #
/// # impl<Ctx: Send> Handler<Ctx> for MyComponent {
/// #   async fn hello(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// #     // ...
/// # }
/// #
///
///
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// This indicates that the current module, referred to with `self`, is the one
/// which had the `generate!` macro expanded.
///
/// If, however, the `generate!` macro was run in a different module then that
/// must be configured:
///
/// ```
/// use futures::stream::TryStreamExt as _;
///
/// mod bindings {
///     wit_bindgen_wrpc::generate!({
///         // ...
/// #   inline: r#"
/// #       package my:test;
/// #
/// #       world my-world {
/// #           export hello: func();
/// #           // ...
/// #       }
/// #   "#,
///     });
/// }
/// #
/// # #[derive(Clone)]
/// # struct MyComponent;
/// #
/// # impl<Ctx: Send> bindings::Handler<Ctx> for MyComponent {
/// #   async fn hello(&self, cx: Ctx) -> anyhow::Result<()> { Ok(()) }
/// #     // ...
/// # }
/// #
/// ;
/// #
/// async fn serve_exports(wrpc: &impl wrpc_transport::Serve) {
///     let invocations = bindings::serve(wrpc, MyComponent).await.unwrap();
///     invocations.into_iter().for_each(|(instance, name, st)| {
///         tokio::spawn(async move {
///             eprintln!("serving {instance} {name}");
///             st.try_collect::<Vec<_>>().await.unwrap();
///         });
///     })
/// }
/// ```
///
/// ## Debugging output to `generate!`
///
/// While `wit-bindgen-wrpc` is tested to the best of our ability there are
/// inevitably bugs and issues that arise. These can range from bad error
/// messages to misconfigured invocations to bugs in the macro itself. To assist
/// with debugging these situations the macro recognizes an environment
/// variable:
///
/// ```shell
/// export WIT_BINDGEN_DEBUG=1
/// ```
///
/// When set the macro will emit the result of expansion to a file and then
/// `include!` that file. Any error messages generated by `rustc` should then
/// point to the generated file and allow you to open it up, read it, and
/// inspect it. This can often provide better context to the error than rustc
/// provides by default with macros.
///
/// It is not recommended to set this environment variable by default as it will
/// cause excessive rebuilds of Cargo projects. It's recommended to only use it
/// as necessary to debug issues.
///
/// ## Options to `generate!`
///
/// The full list of options that can be passed to the `generate!` macro are as
/// follows. Note that there are no required options, they all have default
/// values.
///
///
/// ```
/// use wit_bindgen_wrpc::generate;
/// # macro_rules! generate { ($($t:tt)*) => () }
///
/// generate!({
///     // The name of the world that bindings are being generated for. If this
///     // is not specified then it's required that the package selected
///     // below has a single `world` in it.
///     world: "my-world",
///
///     // Path to parse WIT and its dependencies from. Defaults to the `wit`
///     // folder adjacent to your `Cargo.toml`.
///     //
///     // This parameter also supports the form of a list, such as:
///     // ["../path/to/wit1", "../path/to/wit2"]
///     // Usually used in testing, our test suite may want to generate code
///     // from wit files located in multiple paths within a single mod, and we
///     // don't want to copy these files again.
///     path: "../path/to/wit",
///
///     // Enables passing "inline WIT". If specified this is the default
///     // package that a world is selected from. Any dependencies that this
///     // inline WIT refers to must be defined in the `path` option above.
///     //
///     // By default this is not specified.
///     inline: "
///         world my-world {
///             import wasi:cli/imports;
///
///             export my-run: func()
///         }
///     ",
///
///     // Additional traits to derive for all defined types. Note that not all
///     // types may be able to implement these traits, such as resources.
///     //
///     // By default this set is empty.
///     additional_derives: [core::cmp::PartialEq, core::cmp::Eq, core::hash::Hash, core::clone::Clone],
///
///     // When generating bindings for interfaces that are not defined in the
///     // same package as `world`, this option can be used to either generate
///     // those bindings or point to already generated bindings.
///     // For example, if your world refers to WASI types then the `wasi` crate
///     // already has generated bindings for all WASI types and structures. In this
///     // situation the key `with` here can be used to use those types
///     // elsewhere rather than regenerating types.
///     //
///     // If, however, your world refers to interfaces for which you don't have
///     // already generated bindings then you can use the special `generate` value
///     // to have those bindings generated.
///     //
///     // The `with` key only supports replacing types at the interface level
///     // at this time.
///     //
///     // When an interface is specified no bindings will be generated at
///     // all. It's assumed bindings are fully generated somewhere else. This is an
///     // indicator that any further references to types defined in these
///     // interfaces should use the upstream paths specified here instead.
///     //
///     // Any unused keys in this map are considered an error.
///     with: {
///         "wasi:io/poll": wasi::io::poll,
///         "some:package/my-interface": generate,
///     },
///
///     // Indicates that all interfaces not present in `with` should be assumed
///     // to be marked with `generate`.
///     generate_all,
///
///     // An optional list of function names to skip generating bindings for.
///     // This is only applicable to imports and the name specified is the name
///     // of the function.
///     skip: ["foo", "bar", "baz"],
///
///     // Configure where the `bitflags` crate is located. By default this
///     // is `wit_bindgen_wrpc::bitflags` which already reexports `bitflags` for
///     // you.
///     bitflags_path: "path::to::bitflags",
///
///     // Whether to generate unused `record`, `enum`, `variant` types.
///     // By default, they will not be generated unless they are used as input
///     // or return value of a function.
///     generate_unused_types: false,
///
///     // A list of "features" which correspond to WIT features to activate
///     // when parsing WIT files. This enables `@unstable` annotations showing
///     // up and having bindings generated for them.
///     //
///     // By default this is an empty list.
///     features: ["foo", "bar", "baz"],
/// });
/// ```
///
/// [WIT package]: https://component-model.bytecodealliance.org/design/packages.html
pub use wit_bindgen_wrpc_rust_macro::generate;

#[cfg(docsrs)]
pub mod examples;

pub use anyhow;
pub use bitflags;
pub use bytes;
pub use futures;
pub use tokio;
pub use tokio_util;
pub use tracing;
pub use wasm_tokio;
pub use wrpc_transport;
