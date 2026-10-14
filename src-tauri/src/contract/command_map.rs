//! Generates src/lib/ipc-contract.generated.ts, the frontend's `CommandMap`,
//! from the commands this crate registers (#347).
//!
//! The source of truth is `tauri::generate_handler![...]` in main.rs and the
//! `#[tauri::command]` signature of each registered function:
//!
//! - the frontend sends the Rust parameter names in camelCase (Tauri's
//!   default; a `rename_all` argument fails here until the generator learns
//!   it);
//! - `State`, `AppHandle` and window parameters are injected by Tauri;
//! - an `Option<T>` parameter may be omitted or `null`;
//! - the command resolves with `T` of its `Result<T, String>`.
//!
//! Rust types map to TypeScript type *names* through [`TYPESCRIPT_NAMES`]
//! and [`JSON_VALUES`]; an unmapped type fails the test. The shapes behind the
//! names are checked by the shared fixtures in contracts/transport/
//! (src/test/transport.test.ts), not here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{FnArg, GenericArgument, ItemFn, Pat, PathArguments, ReturnType, Token, Type};

use super::{blessing, repo_root, BLESS_ENV};

/// The generated file, relative to the repository root.
const GENERATED: &str = "src/lib/ipc-contract.generated.ts";

/// Parameter types Tauri injects instead of reading the argument object.
const INJECTED: &[&str] = &["State", "AppHandle", "Window", "WebviewWindow", "Webview"];

/// Rust type name → TypeScript type and the src/lib module that exports it
/// (`None`: a built-in TypeScript type).
const TYPESCRIPT_NAMES: &[(&str, &str, Option<&str>)] = &[
    ("AppState", "AppState", Some("./api")),
    (
        "AudioAttachment",
        "MeetingAudioAttachment",
        Some("./meeting-types"),
    ),
    ("BuildInfo", "BuildInfo", Some("./api")),
    ("CorrectionFile", "CorrectionFile", Some("./meeting-types")),
    (
        "CorrectionOperation",
        "CorrectionOperation",
        Some("./meeting-types"),
    ),
    ("EngineStatus", "EngineStatus", Some("./api")),
    (
        "ExportFormat",
        "MeetingExportFormat",
        Some("./meeting-types"),
    ),
    ("GlossarySuggestion", "GlossarySuggestion", Some("./api")),
    ("HotkeyMode", "HotkeyMode", Some("./api")),
    ("HotkeyProfile", "HotkeyProfile", Some("./api")),
    ("HotkeyStatus", "HotkeyStatus", Some("./api")),
    ("Language", "Language", Some("./api")),
    ("LoadedModelInfo", "LoadedModelInfo", Some("./api")),
    (
        "MeetingReprocessingProposal",
        "MeetingReprocessingProposal",
        Some("./meeting-reprocessing-types"),
    ),
    ("MeetingReview", "MeetingReview", Some("./meeting-types")),
    ("MeetingSnapshot", "MeetingJobSnapshot", Some("./api")),
    (
        "MeetingTranscript",
        "MeetingTranscript",
        Some("./meeting-types"),
    ),
    ("ModelInfo", "WhisperModel", Some("./api")),
    (
        "ProposalState",
        "ProposalState",
        Some("./meeting-reprocessing-types"),
    ),
    (
        "ReprocessingMode",
        "ReprocessingMode",
        Some("./meeting-reprocessing-types"),
    ),
    (
        "ReprocessingRequest",
        "ReprocessingRequest",
        Some("./meeting-reprocessing-types"),
    ),
    ("ReviewState", "MeetingReviewState", Some("./meeting-types")),
    (
        "SelectedReprocessingPlan",
        "SelectedReprocessingPlan",
        Some("./meeting-reprocessing-types"),
    ),
    ("Settings", "Settings", Some("./api")),
    ("TrainingTranscript", "TrainingTranscript", Some("./api")),
    // The frontend passes model ids as strings; Rust rejects unknown ones.
    ("WhisperModel", "string", None),
];

/// Untyped JSON (`serde_json::Value`) by command and position (`result`, or
/// a parameter name): the type the frontend reads or writes it as. Named
/// exceptions: Rust does not check these shapes.
const JSON_VALUES: &[(&str, &str, &str, &str)] = &[
    (
        "get_engine_load_state",
        "result",
        "EngineLoadState",
        "./api",
    ),
    (
        "load_update_recovery",
        "result",
        "UpdateRecoveryPayload",
        "./update-recovery",
    ),
    (
        "save_update_recovery",
        "payload",
        "UpdateRecoveryPayload",
        "./update-recovery",
    ),
];

/// A TypeScript type expression and the named types it imports.
#[derive(Debug, Default)]
struct TsType {
    text: String,
    imports: BTreeSet<(&'static str, &'static str)>,
}

impl TsType {
    fn builtin(text: &str) -> Self {
        Self {
            text: text.into(),
            imports: BTreeSet::new(),
        }
    }

    /// For use inside `T[]`: parenthesized when it is a union.
    fn element(&self) -> String {
        if self.text.contains(" | ") {
            format!("({})", self.text)
        } else {
            self.text.clone()
        }
    }
}

struct Param {
    rust_name: String,
    key: String,
    optional: bool,
    rust_type: String,
    ts: TsType,
}

struct Command {
    module: String,
    name: String,
    params: Vec<Param>,
    rust_result: String,
    result: TsType,
}

/// Everything the generator learned, and every problem it found.
#[derive(Default)]
struct Generation {
    commands: Vec<Command>,
    errors: Vec<String>,
    used_names: BTreeSet<&'static str>,
    used_json: BTreeSet<(&'static str, &'static str)>,
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn parse_file(path: &Path) -> syn::File {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    syn::parse_file(&text)
        .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()))
}

/// Tauri's argument key: `file_path` → `filePath` (heck's lowerCamelCase).
fn camel_case(name: &str) -> String {
    let mut words = name.split('_').filter(|word| !word.is_empty());
    let mut key = words.next().unwrap_or_default().to_string();
    for word in words {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            key.extend(first.to_uppercase());
            key.push_str(chars.as_str());
        }
    }
    key
}

/// The generic arguments of a path segment that are types.
fn type_arguments(arguments: &PathArguments) -> Vec<&Type> {
    match arguments {
        PathArguments::AngleBracketed(generic) => generic
            .args
            .iter()
            .filter_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The Rust type as written, for the generated doc comments.
fn rust_type(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .iter()
            .map(|segment| {
                let arguments = match &segment.arguments {
                    PathArguments::AngleBracketed(generic) => {
                        let inner: Vec<String> = generic
                            .args
                            .iter()
                            .map(|argument| match argument {
                                GenericArgument::Type(ty) => rust_type(ty),
                                GenericArgument::Lifetime(lifetime) => {
                                    format!("'{}", lifetime.ident)
                                }
                                _ => "?".into(),
                            })
                            .collect();
                        format!("<{}>", inner.join(", "))
                    }
                    _ => String::new(),
                };
                format!("{}{arguments}", segment.ident)
            })
            .collect::<Vec<_>>()
            .join("::"),
        Type::Tuple(tuple) if tuple.elems.is_empty() => "()".into(),
        Type::Reference(reference) => format!("&{}", rust_type(&reference.elem)),
        _ => "?".into(),
    }
}

fn last_segment(ty: &Type) -> Option<&syn::PathSegment> {
    match ty {
        Type::Path(path) if path.qself.is_none() => path.path.segments.last(),
        _ => None,
    }
}

impl Generation {
    /// Maps one Rust type at `position` (`result` or a parameter name) of
    /// `command` to TypeScript.
    fn ts_type(&mut self, ty: &Type, command: &str, position: &str) -> Result<TsType, String> {
        if let Type::Tuple(tuple) = ty {
            if tuple.elems.is_empty() && position == "result" {
                return Ok(TsType::builtin("void"));
            }
        }
        let segment =
            last_segment(ty).ok_or_else(|| format!("unsupported type `{}`", rust_type(ty)))?;
        let name = segment.ident.to_string();
        let arguments = type_arguments(&segment.arguments);
        let only_argument = || -> Result<&Type, String> {
            match arguments.as_slice() {
                [inner] => Ok(*inner),
                _ => Err(format!("`{}` needs one type argument", rust_type(ty))),
            }
        };
        match name.as_str() {
            "Option" => {
                let inner = self.ts_type(only_argument()?, command, position)?;
                Ok(TsType {
                    text: format!("{} | null", inner.text),
                    imports: inner.imports,
                })
            }
            "Vec" => {
                let inner = self.ts_type(only_argument()?, command, position)?;
                Ok(TsType {
                    text: format!("{}[]", inner.element()),
                    imports: inner.imports,
                })
            }
            "String" | "str" => Ok(TsType::builtin("string")),
            "bool" => Ok(TsType::builtin("boolean")),
            "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
            | "f32" | "f64" => Ok(TsType::builtin("number")),
            "Value" => {
                let &(command, position, ts, module) = JSON_VALUES
                    .iter()
                    .find(|(c, p, _, _)| *c == command && *p == position)
                    .ok_or_else(|| {
                        format!(
                            "untyped JSON at `{position}`: name the TypeScript type it is in \
                             JSON_VALUES (src-tauri/src/contract/command_map.rs), or give it a \
                             Rust type"
                        )
                    })?;
                self.used_json.insert((command, position));
                Ok(TsType {
                    text: ts.into(),
                    imports: [(module, ts)].into(),
                })
            }
            _ => {
                if !arguments.is_empty() {
                    return Err(format!("unsupported generic type `{}`", rust_type(ty)));
                }
                let &(rust, ts, module) = TYPESCRIPT_NAMES
                    .iter()
                    .find(|(rust, _, _)| *rust == name)
                    .ok_or_else(|| {
                        format!(
                            "no TypeScript type for `{name}`: add it to TYPESCRIPT_NAMES \
                             (src-tauri/src/contract/command_map.rs)"
                        )
                    })?;
                self.used_names.insert(rust);
                Ok(TsType {
                    text: ts.into(),
                    imports: module.map(|module| (module, ts)).into_iter().collect(),
                })
            }
        }
    }

    /// Reads one registered command's signature.
    fn command(&mut self, module: &str, function: &ItemFn) -> Result<Command, String> {
        let name = function.sig.ident.to_string();
        let mut params = Vec::new();
        for input in &function.sig.inputs {
            let FnArg::Typed(typed) = input else {
                return Err("commands are free functions, not methods".into());
            };
            let Pat::Ident(ident) = typed.pat.as_ref() else {
                return Err("parameters must be plain names".into());
            };
            let segment = last_segment(&typed.ty).map(|segment| segment.ident.to_string());
            if segment
                .as_deref()
                .is_some_and(|segment| INJECTED.contains(&segment))
            {
                continue;
            }
            let rust_name = ident.ident.to_string();
            if rust_name.starts_with('_') {
                return Err(format!(
                    "parameter `{rust_name}` is not injected and starts with _"
                ));
            }
            let ts = self.ts_type(&typed.ty, &name, &rust_name)?;
            params.push(Param {
                key: camel_case(&rust_name),
                optional: segment.as_deref() == Some("Option"),
                rust_type: rust_type(&typed.ty),
                rust_name,
                ts,
            });
        }
        let ReturnType::Type(_, output) = &function.sig.output else {
            return Err("commands return a value; `()` is `Result<(), String>`".into());
        };
        let rust_result = rust_type(output);
        let value = match last_segment(output) {
            Some(segment) if segment.ident == "Result" => {
                match type_arguments(&segment.arguments).as_slice() {
                    [value, error] if rust_type(error) == "String" => *value,
                    _ => {
                        return Err(format!(
                            "result `{rust_result}`: only `Result<T, String>` is mapped yet \
                             (typed errors are #360)"
                        ))
                    }
                }
            }
            _ => output.as_ref(),
        };
        let result = self.ts_type(value, &name, "result")?;
        Ok(Command {
            module: module.into(),
            name,
            params,
            rust_result,
            result,
        })
    }
}

/// Whether an attribute is `#[tauri::command]` (or `#[command]`); rejects
/// arguments that would change the wire names.
fn command_attribute(attribute: &syn::Attribute) -> Result<bool, String> {
    let path: Vec<String> = attribute
        .path()
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();
    if path != ["tauri", "command"] && path != ["command"] {
        return Ok(false);
    }
    if let syn::Meta::List(_) = &attribute.meta {
        let mut unsupported = Vec::new();
        attribute
            .parse_nested_meta(|meta| {
                if !meta.path.is_ident("async") {
                    unsupported.push(
                        meta.path
                            .get_ident()
                            .map_or_else(|| "?".into(), ToString::to_string),
                    );
                    // Skip `= value`, if any, so parsing can continue.
                    if let Ok(value) = meta.value() {
                        let _: syn::Expr = value.parse()?;
                    }
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        if !unsupported.is_empty() {
            return Err(format!(
                "#[tauri::command({})] is not supported by the CommandMap generator",
                unsupported.join(", ")
            ));
        }
    }
    Ok(true)
}

/// Every `#[tauri::command]` function in one parsed file, by name.
#[derive(Default)]
struct CommandFunctions {
    found: BTreeMap<String, ItemFn>,
    errors: Vec<String>,
}

impl Visit<'_> for CommandFunctions {
    fn visit_item_fn(&mut self, function: &ItemFn) {
        let mut is_command = false;
        for attribute in &function.attrs {
            match command_attribute(attribute) {
                Ok(found) => is_command |= found,
                Err(error) => self.errors.push(format!("{}: {error}", function.sig.ident)),
            }
        }
        if is_command {
            self.found
                .insert(function.sig.ident.to_string(), function.clone());
        }
    }
}

/// The paths listed in `tauri::generate_handler![...]`.
#[derive(Default)]
struct Handlers {
    lists: Vec<Vec<syn::Path>>,
}

impl Visit<'_> for Handlers {
    fn visit_macro(&mut self, mac: &syn::Macro) {
        if mac
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "generate_handler")
        {
            let paths = mac
                .parse_body_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
                .expect("generate_handler! lists paths");
            self.lists.push(paths.into_iter().collect());
        }
        syn::visit::visit_macro(self, mac);
    }
}

/// Rust source files below `dir`, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("readable directory entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// The module a file defines, as `generate_handler!` paths name it.
fn module_of(path: &Path) -> String {
    let relative = path.strip_prefix(src_dir()).expect("a file below src/");
    let mut parts: Vec<String> = relative
        .iter()
        .map(|part| part.to_string_lossy().trim_end_matches(".rs").to_string())
        .collect();
    if parts.last().is_some_and(|last| last == "mod") {
        parts.pop();
    }
    parts.join("::")
}

fn generate() -> Generation {
    let mut generation = Generation::default();
    let mut handlers = Handlers::default();
    handlers.visit_file(&parse_file(&src_dir().join("main.rs")));
    let [registered] = handlers.lists.as_slice() else {
        panic!(
            "expected one tauri::generate_handler! in main.rs, found {}",
            handlers.lists.len()
        );
    };

    let mut functions: BTreeMap<String, BTreeMap<String, ItemFn>> = BTreeMap::new();
    for file in rust_files(&src_dir()) {
        let mut visitor = CommandFunctions::default();
        visitor.visit_file(&parse_file(&file));
        generation.errors.extend(visitor.errors);
        if !visitor.found.is_empty() {
            functions.insert(module_of(&file), visitor.found);
        }
    }

    for path in registered {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let (module, name) = match segments.as_slice() {
            [name] => ("main".to_string(), name.clone()),
            [module @ .., name] => (module.join("::"), name.clone()),
            [] => unreachable!("syn paths have a segment"),
        };
        let Some(function) = functions
            .get_mut(&module)
            .and_then(|found| found.remove(&name))
        else {
            generation.errors.push(format!(
                "{module}::{name} is registered but is not a #[tauri::command] in src/{module}.rs"
            ));
            continue;
        };
        match generation.command(&module, &function) {
            Ok(command) => generation.commands.push(command),
            Err(error) => generation.errors.push(format!("{module}::{name}: {error}")),
        }
    }
    for (module, unregistered) in &functions {
        for name in unregistered.keys() {
            generation.errors.push(format!(
                "{module}::{name} is a #[tauri::command] but missing from \
                 tauri::generate_handler! in main.rs: the frontend could not call it"
            ));
        }
    }
    generation.commands.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in generation.commands.windows(2) {
        if pair[0].name == pair[1].name {
            generation
                .errors
                .push(format!("two commands are named {}", pair[0].name));
        }
    }
    for (rust, _, _) in TYPESCRIPT_NAMES {
        if !generation.used_names.contains(rust) {
            generation.errors.push(format!(
                "TYPESCRIPT_NAMES maps `{rust}`, which no command uses"
            ));
        }
    }
    for (command, position, _, _) in JSON_VALUES {
        if !generation.used_json.contains(&(*command, *position)) {
            generation.errors.push(format!(
                "JSON_VALUES names {command} `{position}`, which is not untyped JSON"
            ));
        }
    }
    generation
}

/// The generated TypeScript module.
fn render(commands: &[Command]) -> String {
    let mut imports: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for command in commands {
        for (module, name) in command
            .params
            .iter()
            .flat_map(|param| &param.ts.imports)
            .chain(&command.result.imports)
        {
            imports.entry(module).or_default().insert(name);
        }
    }
    let mut out = String::new();
    out.push_str(
        "// Generated from the Rust command signatures by src-tauri/src/contract/command_map.rs\n\
         // (#347). Do not edit: after changing a command, run\n\
         // `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract` in src-tauri/.\n\
         // The app crate's tests fail while this file is out of date.\n",
    );
    for (module, names) in &imports {
        let names: Vec<&str> = names.iter().copied().collect();
        out.push_str(&format!(
            "import type {{ {} }} from \"{module}\";\n",
            names.join(", ")
        ));
    }
    out.push_str(
        "\n/** No arguments. */\n\
         type NoArgs = Record<string, never>;\n\
         \n\
         /**\n \
         * Every command the backend registers (`tauri::generate_handler!` in\n \
         * src-tauri/src/main.rs): the argument object the frontend sends, keyed by\n \
         * the Rust parameter names in camelCase (an `Option` parameter may be\n \
         * omitted or null), and the value the command resolves with. A command\n \
         * that fails rejects with its error string.\n \
         */\n\
         export interface CommandMap {\n",
    );
    for command in commands {
        let signature: Vec<String> = command
            .params
            .iter()
            .map(|param| format!("{}: {}", param.rust_name, param.rust_type))
            .collect();
        out.push_str(&format!(
            "  /** `{}::{}({}) -> {}` */\n",
            command.module,
            command.name,
            signature.join(", "),
            command.rust_result
        ));
        let args = if command.params.is_empty() {
            "NoArgs".to_string()
        } else {
            let fields: Vec<String> = command
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}{}: {}",
                        param.key,
                        if param.optional { "?" } else { "" },
                        param.ts.text
                    )
                })
                .collect();
            format!("{{ {} }}", fields.join("; "))
        };
        out.push_str(&format!(
            "  {}: {{ args: {args}; result: {} }};\n",
            command.name, command.result.text
        ));
    }
    out.push_str(
        "}\n\n/** The registered command names, sorted. */\nexport const COMMAND_NAMES = [\n",
    );
    for command in commands {
        out.push_str(&format!("  \"{}\",\n", command.name));
    }
    out.push_str("] as const satisfies readonly (keyof CommandMap)[];\n");
    out
}

#[test]
fn command_map_is_generated_from_the_rust_signatures() {
    let generation = generate();
    assert!(
        generation.errors.is_empty(),
        "the CommandMap generator found problems:\n  {}",
        generation.errors.join("\n  ")
    );
    let generated = render(&generation.commands);
    let path = repo_root().join(GENERATED);
    if blessing() {
        fs::write(&path, &generated)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        return;
    }
    let checked_in = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n");
    if checked_in != generated {
        let difference = checked_in
            .lines()
            .zip(generated.lines())
            .position(|(have, want)| have != want)
            .map_or_else(
                || "the files differ in length".to_string(),
                |index| {
                    format!(
                        "first difference on line {}:\n  checked in: {}\n  generated:  {}",
                        index + 1,
                        checked_in.lines().nth(index).unwrap_or_default(),
                        generated.lines().nth(index).unwrap_or_default()
                    )
                },
            );
        panic!(
            "{GENERATED} is out of date with the Rust commands ({difference}).\n\
             Regenerate with `{BLESS_ENV}=1 cargo test -p sagascript contract` in src-tauri/, \
             then update the wrapper in src/lib/api.ts and its row in src/lib/api.test.ts."
        );
    }
}

#[test]
fn argument_keys_are_tauri_camel_case() {
    assert_eq!(camel_case("file_path"), "filePath");
    assert_eq!(camel_case("source_sha256"), "sourceSha256");
    assert_eq!(camel_case("language"), "language");
    assert_eq!(camel_case("expected_revision"), "expectedRevision");
}

#[test]
fn signatures_map_or_fail_with_a_fix() {
    let mut generation = Generation::default();
    let function: ItemFn = syn::parse_quote! {
        #[tauri::command]
        pub async fn example(
            app: tauri::AppHandle,
            controller: State<'_, SharedController>,
            unknown: Mystery,
        ) -> Result<(), String> { todo!() }
    };
    let error = generation
        .command("commands", &function)
        .err()
        .expect("an unmapped type is an error");
    assert!(
        error.contains("no TypeScript type for `Mystery`"),
        "{error}"
    );

    let function: ItemFn = syn::parse_quote! {
        #[tauri::command]
        pub async fn example(
            controller: State<'_, SharedController>,
            profile_id: Option<String>,
        ) -> Result<Vec<ModelInfo>, String> { todo!() }
    };
    let command = generation
        .command("commands", &function)
        .expect("a mapped signature");
    assert_eq!(command.params.len(), 1, "State is injected, not sent");
    assert_eq!(command.params[0].key, "profileId");
    assert!(command.params[0].optional);
    assert_eq!(command.params[0].ts.text, "string | null");
    assert_eq!(command.result.text, "WhisperModel[]");

    let function: ItemFn = syn::parse_quote! {
        #[tauri::command]
        pub async fn example() -> Result<(), CommandError> { todo!() }
    };
    assert!(generation.command("commands", &function).is_err());

    let renamed: syn::Attribute = syn::parse_quote!(#[tauri::command(rename_all = "snake_case")]);
    assert!(command_attribute(&renamed).is_err());
    let asynchronous: syn::Attribute = syn::parse_quote!(#[tauri::command(async)]);
    assert_eq!(command_attribute(&asynchronous), Ok(true));
}
