use ignore::gitignore::GitignoreBuilder;
use proc_macro::{TokenStream, TokenTree};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use witx_bindgen_gen_core::witx2::abi::Direction;
use witx_bindgen_gen_core::Generator;

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-rust-wasm")]
pub fn rust_wasm_import(input: TokenStream) -> TokenStream {
    gen_rust(
        input,
        Direction::Import,
        &[
            (
                "import",
                || witx_bindgen_gen_rust_wasm::Opts::default().build(),
                |_| quote::quote!(),
            ),
            (
                "import-unchecked",
                || {
                    let mut opts = witx_bindgen_gen_rust_wasm::Opts::default();
                    opts.unchecked = true;
                    opts.build()
                },
                |_| quote::quote!(),
            ),
        ],
    )
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-rust-wasm")]
pub fn rust_wasm_export(input: TokenStream) -> TokenStream {
    use heck::*;
    use std::collections::BTreeMap;
    use witx2::{FunctionKind, Type, TypeDefKind};

    return gen_rust(
        input,
        Direction::Export,
        &[
            (
                "export",
                || witx_bindgen_gen_rust_wasm::Opts::default().build(),
                gen_extra,
            ),
            (
                "export-unchecked",
                || {
                    let mut opts = witx_bindgen_gen_rust_wasm::Opts::default();
                    opts.unchecked = true;
                    opts.symbol_namespace = "unchecked".to_string();
                    opts.build()
                },
                gen_extra,
            ),
        ],
    );

    fn gen_extra(iface: &witx2::Interface) -> proc_macro2::TokenStream {
        let mut ret = quote::quote!();
        if iface.resources.len() == 0 && iface.functions.len() == 0 {
            return ret;
        }

        let snake = quote::format_ident!("{}", iface.name.to_snake_case());
        let camel = quote::format_ident!("{}", iface.name.to_camel_case());

        for (_, r) in iface.resources.iter() {
            let name = quote::format_ident!("{}", r.name.to_camel_case());
            ret.extend(quote::quote!(pub struct #name;));
        }

        let mut methods = Vec::new();
        let mut resources = BTreeMap::new();

        for f in iface.functions.iter() {
            let name = quote::format_ident!("{}", f.item_name().to_snake_case());
            let mut params = f
                .params
                .iter()
                .map(|(_, t)| quote_ty(true, iface, t))
                .collect::<Vec<_>>();
            let mut results = f.results.iter().map(|(_, t)| quote_ty(false, iface, t));
            let ret = match f.results.len() {
                0 => quote::quote! { () },
                1 => results.next().unwrap(),
                _ => quote::quote! { (#(#results),*) },
            };
            let mut self_ = quote::quote!();
            if let FunctionKind::Method { .. } = &f.kind {
                params.remove(0);
                self_ = quote::quote!(&self,);
            }
            let method = quote::quote! {
                fn #name(#self_ #(_: #params),*) -> #ret {
                    loop {}
                }
            };
            match &f.kind {
                FunctionKind::Freestanding => methods.push(method),
                FunctionKind::Static { resource, .. } | FunctionKind::Method { resource, .. } => {
                    resources
                        .entry(*resource)
                        .or_insert(Vec::new())
                        .push(method);
                }
            }
        }
        ret.extend(quote::quote! {
            struct #camel;
            impl #snake::#camel for #camel {
                #(#methods)*
            }
        });
        for (id, methods) in resources {
            let name = quote::format_ident!("{}", iface.resources[id].name.to_camel_case());
            ret.extend(quote::quote! {
                impl #snake::#name for #name {
                    #(#methods)*
                }
            });
        }

        ret
    }

    fn quote_ty(
        param: bool,
        iface: &witx2::Interface,
        ty: &witx2::Type,
    ) -> proc_macro2::TokenStream {
        match *ty {
            Type::U8 => quote::quote! { u8 },
            Type::S8 => quote::quote! { i8 },
            Type::U16 => quote::quote! { u16 },
            Type::S16 => quote::quote! { i16 },
            Type::U32 => quote::quote! { u32 },
            Type::S32 => quote::quote! { i32 },
            Type::U64 => quote::quote! { u64 },
            Type::S64 => quote::quote! { i64 },
            Type::CChar => quote::quote! { u8 },
            Type::Usize => quote::quote! { usize },
            Type::F32 => quote::quote! { f32 },
            Type::F64 => quote::quote! { f64 },
            Type::Char => quote::quote! { char },
            Type::Handle(resource) => {
                let name =
                    quote::format_ident!("{}", iface.resources[resource].name.to_camel_case());
                quote::quote! { witx_bindgen_rust::Handle<#name> }
            }
            Type::Id(id) => quote_id(param, iface, id),
        }
    }

    fn quote_id(
        param: bool,
        iface: &witx2::Interface,
        id: witx2::TypeId,
    ) -> proc_macro2::TokenStream {
        let ty = &iface.types[id];
        if let Some(name) = &ty.name {
            let name = quote::format_ident!("{}", name.to_camel_case());
            let module = quote::format_ident!("{}", iface.name.to_snake_case());
            return quote::quote! { #module::#name };
        }
        match &ty.kind {
            TypeDefKind::Type(t) => quote_ty(param, iface, t),
            TypeDefKind::Pointer(t) => {
                let t = quote_ty(param, iface, t);
                quote::quote! { *mut #t }
            }
            TypeDefKind::ConstPointer(t) => {
                let t = quote_ty(param, iface, t);
                quote::quote! { *const #t }
            }
            TypeDefKind::List(t) => {
                if *t == Type::Char {
                    quote::quote! { String }
                } else {
                    let t = quote_ty(param, iface, t);
                    quote::quote! { Vec<#t> }
                }
            }
            TypeDefKind::PushBuffer(_) => panic!("unimplemented push-buffer"),
            TypeDefKind::PullBuffer(_) => panic!("unimplemented pull-buffer"),
            TypeDefKind::Record(r) => {
                let fields = r.fields.iter().map(|f| quote_ty(param, iface, &f.ty));
                quote::quote! { (#(#fields,)*) }
            }
            TypeDefKind::Variant(v) => {
                if v.is_bool() {
                    quote::quote! { bool }
                } else if let Some(ty) = v.as_option() {
                    let ty = quote_ty(param, iface, ty);
                    quote::quote! { Option<#ty> }
                } else if let Some((ok, err)) = v.as_expected() {
                    let ok = match ok {
                        Some(ok) => quote_ty(param, iface, ok),
                        None => quote::quote! { () },
                    };
                    let err = match err {
                        Some(err) => quote_ty(param, iface, err),
                        None => quote::quote! { () },
                    };
                    quote::quote! { Result<#ok, #err> }
                } else {
                    panic!("unknown variant");
                }
            }
        }
    }
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-wasmtime")]
pub fn wasmtime_import(input: TokenStream) -> TokenStream {
    gen_rust(
        input,
        Direction::Import,
        &[
            (
                "import",
                || witx_bindgen_gen_wasmtime::Opts::default().build(),
                |_| quote::quote!(),
            ),
            (
                "import-tracing-and-custom-error",
                || {
                    let mut opts = witx_bindgen_gen_wasmtime::Opts::default();
                    opts.tracing = true;
                    opts.custom_error = true;
                    opts.build()
                },
                |_| quote::quote!(),
            ),
            (
                "import-async",
                || {
                    let mut opts = witx_bindgen_gen_wasmtime::Opts::default();
                    opts.async_ = witx_bindgen_gen_wasmtime::Async::All;
                    opts.build()
                },
                |_| quote::quote!(),
            ),
        ],
    )
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-wasmtime")]
pub fn wasmtime_export(input: TokenStream) -> TokenStream {
    gen_rust(
        input,
        Direction::Export,
        &[
            (
                "export",
                || witx_bindgen_gen_wasmtime::Opts::default().build(),
                |_| quote::quote!(),
            ),
            (
                "export-async",
                || {
                    let mut opts = witx_bindgen_gen_wasmtime::Opts::default();
                    opts.async_ = witx_bindgen_gen_wasmtime::Async::All;
                    opts.build()
                },
                |_| quote::quote!(),
            ),
        ],
    )
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-js")]
pub fn js_import(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Import, "import", || {
        witx_bindgen_gen_js::Opts::default().build()
    })
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-js")]
pub fn js_export(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Export, "export", || {
        witx_bindgen_gen_js::Opts::default().build()
    })
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-c")]
pub fn c_import(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Import, "import", || {
        witx_bindgen_gen_c::Opts::default().build()
    })
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-c")]
pub fn c_export(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Export, "export", || {
        witx_bindgen_gen_c::Opts::default().build()
    })
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-wasmtime-py")]
pub fn py_import(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Import, "import", || {
        witx_bindgen_gen_wasmtime_py::Opts::default().build()
    })
}

#[proc_macro]
#[cfg(feature = "witx-bindgen-gen-wasmtime-py")]
pub fn py_export(input: TokenStream) -> TokenStream {
    gen_verify(input, Direction::Export, "export", || {
        witx_bindgen_gen_wasmtime_py::Opts::default().build()
    })
}

fn generate_tests<G>(
    input: TokenStream,
    dir: &str,
    mkgen: impl Fn(&Path) -> (G, Direction),
) -> Vec<(witx2::Interface, PathBuf, PathBuf)>
where
    G: Generator,
{
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            eprintln!("panic: {:?}", backtrace::Backtrace::new());
            prev(info);
        }));
    });

    let mut builder = GitignoreBuilder::new("tests");
    for token in input {
        let lit = match token {
            TokenTree::Literal(l) => l.to_string(),
            _ => panic!("invalid input"),
        };
        assert!(lit.starts_with("\""));
        assert!(lit.ends_with("\""));
        builder.add_line(None, &lit[1..lit.len() - 1]).unwrap();
    }
    let ignore = builder.build().unwrap();
    let tests = ignore::Walk::new("tests").filter_map(|d| {
        let d = d.unwrap();
        let path = d.path();
        match ignore.matched(path, d.file_type().map(|d| d.is_dir()).unwrap_or(false)) {
            ignore::Match::None => None,
            ignore::Match::Ignore(_) => Some(d.into_path()),
            ignore::Match::Whitelist(_) => None,
        }
    });
    let mut out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    out_dir.push(dir);
    let mut sources = Vec::new();
    let cwd = env::current_dir().unwrap();
    for test in tests {
        let (mut gen, dir) = mkgen(&test);
        if dir == Direction::Export {
            match test.file_name().unwrap().to_str().unwrap() {
                "wasi_snapshot_preview1.witx" | "typenames.witx" | "legacy.witx" => continue,
                _ => {}
            }
        }
        let mut files = Default::default();
        let iface = witx2::Interface::parse_file(&test).unwrap();
        gen.generate(&iface, dir, &mut files);

        let dst = out_dir.join(test.file_stem().unwrap());
        drop(fs::remove_dir_all(&dst));
        fs::create_dir_all(&dst).unwrap();
        for (file, contents) in files.iter() {
            fs::write(dst.join(file), contents).unwrap();
        }
        sources.push((iface, dst, cwd.join(test)));
    }
    sources
}

#[allow(dead_code)]
fn gen_rust<G: Generator>(
    // input to the original procedural macro
    input: TokenStream,
    // whether we're generating bindings for imports or exports
    dir: Direction,
    // a list of tests, tuples of:
    //  * name of the test (directory to generate code into)
    //  * method to create the `G` which will generate code
    //  * method to generate auxiliary tokens to place in the module,
    //    optionally.
    tests: &[(
        &'static str,
        fn() -> G,
        fn(&witx2::Interface) -> proc_macro2::TokenStream,
    )],
) -> TokenStream {
    let mut ret = proc_macro2::TokenStream::new();
    let mut rustfmt = std::process::Command::new("rustfmt");
    rustfmt.arg("--edition=2018");
    for (name, mk, extra) in tests {
        let tests = generate_tests(input.clone(), name, |_path| (mk(), dir));
        let mut sources = proc_macro2::TokenStream::new();
        for (iface, gen_dir, _input_witx) in tests.iter() {
            let test = gen_dir.join("bindings.rs");
            rustfmt.arg(&test);
            let test = test.display().to_string();
            sources.extend(quote::quote!(include!(#test);));
            let extra = extra(iface);
            if extra.is_empty() {
                continue;
            }
            let test = gen_dir.join("extra.rs");
            rustfmt.arg(&test);
            let test = test.display().to_string();
            sources.extend(quote::quote!(include!(#test);));
            fs::write(&test, extra.to_string()).unwrap();
        }
        let name = quote::format_ident!("{}", name.replace("-", "_"));
        ret.extend(quote::quote!( mod #name { #sources } ));
    }
    drop(rustfmt.output());
    ret.into()
}

#[allow(dead_code)]
fn gen_verify<G: Generator>(
    input: TokenStream,
    dir: Direction,
    name: &str,
    mkgen: fn() -> G,
) -> TokenStream {
    use heck::*;

    let tests = generate_tests(input, name, |_path| (mkgen(), dir));
    let tests = tests.iter().map(|(iface, test, witx)| {
        let test = test.display().to_string();
        let witx = witx.display().to_string();
        let name = quote::format_ident!("{}", iface.name.to_snake_case());
        let iface_name = iface.name.to_snake_case();
        quote::quote! {
            #[test]
            fn #name() {
                const _: &str = include_str!(#witx);
                crate::verify(#test, #iface_name);
            }
        }
    });
    (quote::quote!(#(#tests)*)).into()
}
