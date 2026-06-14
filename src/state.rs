#![allow(dead_code)]
use arc_swap::ArcSwap;
use dashmap::{DashMap, DashSet};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_lsp_server::ls_types::{Position, Range, Uri};
use tree_sitter::Tree;

use crate::config::ResolvedConfig;

// ── Pass-1 facts ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileFacts {
    pub uri: Uri,
    pub apps: Vec<AppDecl>,
    pub routers: Vec<RouterDecl>,
    pub includes: Vec<IncludeCall>,
    pub routes: Vec<RouteFact>,
    pub dep_defs: Vec<DepDef>,
    pub dep_refs: Vec<DepRef>,
    pub templates: Vec<TemplateRef>,
    pub template_envs: Vec<TemplateEnvDecl>,
    pub models: Vec<ModelFact>,
    pub client_calls: Vec<ClientCall>,
    pub middlewares: Vec<MiddlewareCall>,
    pub mw_classes: Vec<MwClassDecl>,
    pub override_sites: Vec<OverrideSite>,
    pub url_for_sites: Vec<UrlForSite>,
    pub env_lookups: Vec<EnvLookupSite>,
    pub env_file_decls: Vec<EnvFileDecl>,
    pub settings_classes: Vec<SettingsClassDecl>,
    /// Names imported into this file's scope (from any import statement).
    /// Used to suppress false-positive model diagnostics for external library symbols.
    pub imported_names: Vec<String>,
    /// Maps imported symbol name → dotted source module path, for `from X import Y` statements.
    /// Used to resolve the target module for "Create model" actions (imports-first targeting).
    pub imported_from: HashMap<String, String>,
    /// Maps alias → original symbol name for aliased imports: `from X import Y as Z` → Z → Y.
    /// Used in the linker to resolve `include_router(alias)` back to the original router name.
    pub import_alias_originals: HashMap<String, String>,
    /// Parameters using `Depends(...)` — either inline (`T = Depends(fn)`) or Annotated-style.
    /// Drives the "Convert to Annotated / Convert to inline" refactor actions.
    pub annotated_params: Vec<AnnotatedParam>,
    /// Module-level type alias name → dep fn name for `X = Annotated[T, Depends(fn)]` patterns.
    /// Used in route_param_checks to resolve plain-typed handler params to their dep fns.
    pub dep_type_aliases: std::collections::HashMap<String, String>,
    /// Range of each dep type alias assignment (same keys as `dep_type_aliases`).
    /// Stored separately so code lenses can position the "N usages" annotation without
    /// changing the value type of `dep_type_aliases`.
    pub dep_type_alias_ranges: std::collections::HashMap<String, Range>,
    /// Handler params with plain identifier types (not `Depends(...)` or `Annotated[...,Depends]`).
    /// Paired with `dep_type_aliases` to detect path params consumed by type-alias deps.
    pub plain_typed_params: Vec<PlainTypedParam>,
}

impl FileFacts {
    pub fn new(uri: Uri) -> Self {
        Self {
            uri,
            apps: vec![],
            routers: vec![],
            includes: vec![],
            routes: vec![],
            dep_defs: vec![],
            dep_refs: vec![],
            templates: vec![],
            template_envs: vec![],
            models: vec![],
            client_calls: vec![],
            middlewares: vec![],
            mw_classes: vec![],
            override_sites: vec![],
            url_for_sites: vec![],
            env_lookups: vec![],
            env_file_decls: vec![],
            settings_classes: vec![],
            imported_names: vec![],
            imported_from: HashMap::new(),
            import_alias_originals: HashMap::new(),
            annotated_params: vec![],
            dep_type_aliases: std::collections::HashMap::new(),
            dep_type_alias_ranges: std::collections::HashMap::new(),
            plain_typed_params: vec![],
        }
    }
}

/// A function parameter with a plain (non-Annotated, non-Depends) type annotation.
/// Captured for dep-type-alias resolution: `project: CurrentProject` where
/// `CurrentProject = Annotated[T, Depends(fn)]` is a module-level alias.
#[derive(Debug, Clone)]
pub struct PlainTypedParam {
    pub containing_func: String,
    pub param_name: String,
    pub type_name: String,
    /// Range of the type annotation identifier in the source (e.g. `DbSession` in `db: DbSession`).
    /// Used to provide reference locations when clicking "N usages" code lenses.
    pub annotation_range: Range,
}

/// A function parameter that uses `Depends(...)` — either inline or `Annotated` style.
#[derive(Debug, Clone)]
pub struct AnnotatedParam {
    /// Used by "Extract dependency" (§3.3) to locate the enclosing function.
    pub containing_func: String,
    pub param_name: String,
    /// True = `Annotated[T, Depends(fn)]`, False = `T = Depends(fn)`
    pub is_annotated: bool,
    /// For inline: range of `T`; for annotated: range of the full `Annotated[...]` expression
    pub annotation_range: Range,
    /// For inline only: range of the `Depends(fn)` call expression
    pub default_range: Option<Range>,
    /// Inner type text, e.g. `"Session"`
    pub type_text: String,
    /// Depends expression text, e.g. `"Depends(get_db)"`
    pub depends_text: String,
    /// True when `Annotated` has extra args beyond `[T, Depends(fn)]`, e.g. `Field()`.
    /// The annotated→inline conversion is suppressed to avoid data loss.
    pub has_extra_args: bool,
}

#[derive(Debug, Clone)]
pub struct AppDecl {
    pub name: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct RouterDecl {
    pub name: String,
    pub prefix: PrefixValue,
    pub tags: Vec<String>,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct IncludeCall {
    pub target: String,
    pub prefix: PrefixValue,
    pub app_name: String,
    pub dependencies: Vec<String>,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub enum PrefixValue {
    Literal(String),
    Unresolved,
}

#[derive(Debug, Clone)]
pub struct RouteFact {
    pub handler_name: String,
    pub handler_range: Range,
    pub object_name: String,
    pub methods: Vec<Method>,
    pub path: PrefixValue,
    pub path_range: Option<Range>,
    /// UTF-16 width of the string prefix + opening quote(s) in the path literal.
    /// e.g. `"` → 1, `r"` → 2, `"""` → 3. `None` when path came from a constant.
    pub path_quote_width: Option<u32>,
    pub response_model: Option<String>,
    pub response_model_range: Option<Range>,
    /// Return type annotation `-> T` from the handler function (bare identifier only).
    /// Used as a fallback response model when `response_model=` kwarg is absent.
    pub return_annotation: Option<String>,
    pub status_code: Option<u16>,
    pub dependencies: Vec<String>,
    pub route_name: Option<String>,
    pub handler_params: Vec<String>,
    /// Per-param source ranges, aligned with `handler_params` (same indices).
    /// Used by the `route/arg-missing-param` rename quick fix.
    pub handler_param_ranges: Vec<Range>,
    /// Position just before the closing `)` of the parameter list.
    /// Used by the `route/param-missing-arg` add-parameter quick fix.
    pub params_insert_pos: Option<tower_lsp_server::ls_types::Position>,
    pub handler_has_splat_args: bool,
    pub handler_params_known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
    WebSocket,
    Mount,
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Method::Get => write!(f, "GET"),
            Method::Post => write!(f, "POST"),
            Method::Put => write!(f, "PUT"),
            Method::Patch => write!(f, "PATCH"),
            Method::Delete => write!(f, "DELETE"),
            Method::Head => write!(f, "HEAD"),
            Method::Options => write!(f, "OPTIONS"),
            Method::Trace => write!(f, "TRACE"),
            Method::WebSocket => write!(f, "WEBSOCKET"),
            Method::Mount => write!(f, "MOUNT"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DepDef {
    pub name: String,
    pub node_id: NodeId,
    pub has_yield: bool,
    pub param_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DepRef {
    pub name: String,
    /// Range of the entire `Depends(...)` expression.
    pub range: Range,
    pub is_called: bool,
    /// When `is_called`, the range of the inner call expression (e.g. `get_db()`).
    /// The remove-call fix replaces this range with just `name`.
    pub callee_range: Option<Range>,
    /// Name of the function or class body that contains this Depends call.
    /// None when at module scope.
    pub containing_func: Option<String>,
    /// NodeId of the containing function's name node.
    /// Set by the parser; used in build_dep_graph to avoid name-based lookup.
    pub caller_node_id: Option<NodeId>,
}

#[derive(Debug, Clone)]
pub struct TemplateRef {
    pub path: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct TemplateEnvDecl {
    pub directories: Vec<String>,
    pub var_name: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct TemplateUrlForSite {
    pub name: String,
    /// Range of the route-name string literal (for diagnostics and completion).
    pub string_range: Range,
    /// Keyword argument names present in the call (values are opaque — only names checked).
    pub kwarg_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModelFact {
    pub name: String,
    pub range: Range,
    pub is_settings: bool,
}

#[derive(Debug, Clone)]
pub struct ClientCall {
    pub fixture_name: String,
    pub method: Method,
    pub path: String,
    /// When true, `path` is a static prefix extracted from a dynamic string (f-string, format call, etc.)
    /// and route matching uses `starts_with` instead of exact equality.
    pub is_prefix: bool,
    /// Total number of path segments inferred from the full dynamic path (when `is_prefix` is true).
    /// Computed by counting slashes across all static content in the path template + 1.
    /// Used by `match_prefix` to filter candidates with the wrong depth.
    pub path_depth: Option<usize>,
    /// Range of the entire call expression (used for goto + diagnostics).
    pub range: Range,
    /// Range of the path string content only (excluding quotes), for completion textEdit.
    pub path_range: Range,
}

#[derive(Debug, Clone)]
pub struct MiddlewareCall {
    pub app_name: String,
    pub source: MwSource,
    pub range: Range,
    /// Position just after the class argument — kwarg completion only fires past this point.
    pub kwargs_start: Option<Position>,
    /// Kwarg names already written in this call, for filtering.
    pub present_kwargs: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum MwSource {
    Class(String),
    DecoratorFn(String),
}

#[derive(Debug, Clone)]
pub struct MwKwarg {
    pub name: String,
    /// Type annotation + default, e.g. `list[str] = []` or `str`.
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MwClassDecl {
    pub class_name: String,
    pub kwargs: Vec<MwKwarg>,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct OverrideSite {
    pub name: String,
    pub range: Range,
}

/// A `request.url_for(name, **kwargs)` or `obj.url_path_for(name, **kwargs)` call site.
#[derive(Debug, Clone)]
pub struct UrlForSite {
    pub name: String,
    pub kwarg_names: Vec<String>,
    /// True when a `**splat` argument was present; param-mismatch is suppressed when set.
    pub has_splat_kwargs: bool,
    pub range: Range,
    /// Range of the string content (excluding quotes) in the name argument; used for completions.
    pub name_range: Option<Range>,
}

/// A recognized env key lookup site (os.environ["KEY"], config("KEY"), etc.)
#[derive(Debug, Clone)]
pub struct EnvLookupSite {
    pub key: String,
    pub has_default: bool,
    pub loader: EnvLoader,
    /// Range of the entire lookup expression.
    pub range: Range,
    /// Range of the key string literal (including quotes).
    pub key_range: Range,
    /// Range covering only the key content (excluding opening/closing quote chars),
    /// ready to use as the textEdit replace range.
    pub replace_range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvLoader {
    OsEnviron,
    StarletteConfig,
    Environs,
    DotenvValues,
}

/// A declaration of an env file path found in code (starlette Config, load_dotenv, etc.)
#[derive(Debug, Clone)]
pub struct EnvFileDecl {
    pub path: String,
    pub loader: LoaderKind,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub enum LoaderKind {
    StarletteConfig,
    PydanticSettings,
    Dotenv,
    Environs,
}

/// A pydantic-settings BaseSettings field binding.
#[derive(Debug, Clone)]
pub struct SettingsField {
    pub field_name: String,
    /// Resolved env key (field_name uppercased + prefix, or explicit alias)
    pub env_key: Option<String>,
    pub has_default: bool,
    pub range: Range,
}

/// A pydantic BaseSettings subclass with its declared fields.
#[derive(Debug, Clone)]
pub struct SettingsClassDecl {
    pub class_name: String,
    /// Direct superclass names as written in source (e.g. `["BaseSettings", "SomeMixin"]`).
    /// Used by the linker to resolve inherited fields across files.
    pub superclass_names: Vec<String>,
    pub env_prefix: Option<String>,
    pub env_file: Option<String>,
    pub fields: Vec<SettingsField>,
    pub range: Range,
}

// ── Pass-2 Linked snapshot ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RouteId(pub String);

impl RouteId {
    pub fn new(uri: &Uri, handler_name: &str, method: &Method) -> Self {
        RouteId(format!("{}:{}:{}", uri.as_str(), handler_name, method))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId {
    pub uri: Uri,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct Location {
    pub uri: Uri,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedPath {
    Resolved(String),
    Unresolved,
}

#[derive(Debug, Clone)]
pub struct PathParam {
    pub name: String,
    pub converter: PathConverter,
}

#[derive(Debug, Clone)]
pub enum PathConverter {
    Str,
    Int,
    Float,
    Uuid,
    Path,
}

#[derive(Debug, Clone)]
pub struct ChainLink {
    pub prefix: String,
    pub object_name: String,
}

#[derive(Debug, Clone)]
pub struct RouteRecord {
    pub id: RouteId,
    pub ordinal: u32,
    pub name: String,
    pub method: Method,
    pub resolved_path: ResolvedPath,
    pub decorator_path: String,
    pub chain: Vec<ChainLink>,
    pub handler: Location,
    pub path_params: Vec<PathParam>,
    pub response_model: Option<String>,
    pub response_model_range: Option<Range>,
    /// Fallback: handler return annotation `-> T` when `response_model=` kwarg is absent.
    pub return_annotation: Option<String>,
    pub dependencies: Vec<String>,
    pub middleware: Vec<String>,
    pub path_range: Option<Range>,
    /// UTF-16 width of the string prefix + opening quote(s) in the path literal.
    pub path_quote_width: Option<u32>,
    pub handler_params: Vec<String>,
    /// Per-param source ranges, aligned with `handler_params`.
    pub handler_param_ranges: Vec<Range>,
    /// Position just before the closing `)` of the parameter list.
    pub params_insert_pos: Option<tower_lsp_server::ls_types::Position>,
    pub handler_has_splat_args: bool,
    /// True for decorator routes (params extracted from source); false for table-style routes.
    pub handler_params_known: bool,
}

#[derive(Debug, Clone)]
pub struct ModelRecord {
    pub name: String,
    pub location: Location,
    pub is_settings: bool,
}

#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub value: String,
    pub locations: Vec<Location>,
    pub from_process_env: bool,
}

#[derive(Debug, Clone)]
pub struct MwInit {
    pub location: Location,
    pub kwargs: Vec<MwKwarg>,
}

#[derive(Debug, Clone)]
pub struct ClientCallSite {
    pub method: Method,
    pub path: String,
    pub location: Location,
}

#[derive(Default)]
pub struct DepGraph {
    pub uses: HashMap<NodeId, Vec<NodeId>>,
    pub used_by: HashMap<NodeId, Vec<NodeId>>,
    pub override_sites: HashMap<NodeId, Vec<Location>>,
}


#[derive(Default)]
pub struct PathTrie {
    pub root: TrieNode,
}

#[derive(Default)]
pub struct TrieNode {
    pub literal: HashMap<String, TrieNode>,
    pub param: Option<(String, Box<TrieNode>)>,
    pub path_param: Option<(String, Box<TrieNode>)>,
    pub routes: Vec<RouteId>,
}


#[derive(Default)]
pub struct Linked {
    pub generation: u64,
    pub route_index: HashMap<RouteId, Vec<RouteRecord>>,
    pub route_names: HashMap<String, Vec<RouteId>>,
    pub path_trie: PathTrie,
    pub dep_graph: DepGraph,
    /// Maps each cycle-member `NodeId` to the ordered cycle it belongs to (REQ-DI-04).
    pub dep_cycle_map: HashMap<NodeId, Vec<NodeId>>,
    pub template_index: HashMap<String, Uri>,
    pub model_index: HashMap<String, Vec<ModelRecord>>,
    pub env_index: HashMap<String, EnvEntry>,
    /// Keys present in any of the configured `settings_env_files`.
    /// Used by `settings/env-key-missing` to check required BaseSettings fields.
    pub env_file_keys: std::collections::HashSet<String>,
    pub middleware_classes: HashMap<String, Vec<MwInit>>,
    pub test_refs: HashMap<RouteId, Vec<ClientCallSite>>,
    /// Inverted index: call-site location → matched RouteIds (REQ-NAV-01 O(1) goto).
    pub call_site_index: HashMap<(Uri, Range), Vec<RouteId>>,
    /// Dependency function names that are "proven" (have a yield or are bare refs).
    /// Precomputed at link time to avoid scanning all file_facts on every diagnostic request.
    pub proven_dep_names: std::collections::HashSet<String>,
    /// dep_name → param_names, precomputed for route/param-missing-arg checks.
    pub dep_params: HashMap<String, Vec<String>>,
}


// ── WorkspaceState ────────────────────────────────────────────────────────────

pub struct WorkspaceState {
    // Pass-1: per-file, concurrent writes
    pub file_facts: DashMap<Uri, FileFacts>,
    pub file_sources: DashMap<Uri, String>,
    pub parse_trees: DashMap<Uri, Tree>,
    pub template_facts: DashMap<Uri, Vec<TemplateUrlForSite>>,
    pub env_file_entries: DashMap<Uri, Vec<crate::parsing::dotenv::DotenvEntry>>,
    pub open_docs: DashSet<Uri>,
    /// Last-applied textDocument version per URI (from didChange).
    /// Used to reject out-of-order or duplicate notifications.
    pub doc_versions: DashMap<Uri, i64>,

    // Pass-2: one immutable snapshot, atomically swapped
    pub linked: ArcSwap<Linked>,

    // Per-URI mutex: serializes document notifications for the same file (REQ-ARCH-08)
    pub doc_locks: DashMap<Uri, Arc<Mutex<()>>>,

    // Debounce: sender bumped on every pass-1 change; pass-2 task watches it
    pub link_tx: tokio::sync::watch::Sender<u64>,
    pub link_rx: tokio::sync::watch::Receiver<u64>,

    pub config: RwLock<ResolvedConfig>,
    pub generation: std::sync::atomic::AtomicU64,
    pub encoding: std::sync::atomic::AtomicU8,
    pub show_document_supported: std::sync::atomic::AtomicBool,
    pub code_lens_refresh_supported: std::sync::atomic::AtomicBool,
    pub file_watch_dynamic_registration: std::sync::atomic::AtomicBool,
    pub work_done_progress_supported: std::sync::atomic::AtomicBool,
    pub can_create_files: std::sync::atomic::AtomicBool,
}

impl WorkspaceState {
    pub fn new(config: ResolvedConfig) -> Arc<Self> {
        let (link_tx, link_rx) = tokio::sync::watch::channel(0u64);
        Arc::new(Self {
            file_facts: DashMap::new(),
            file_sources: DashMap::new(),
            parse_trees: DashMap::new(),
            template_facts: DashMap::new(),
            env_file_entries: DashMap::new(),
            open_docs: DashSet::new(),
            doc_versions: DashMap::new(),
            linked: ArcSwap::new(Arc::new(Linked::default())),
            doc_locks: DashMap::new(),
            link_tx,
            link_rx,
            config: RwLock::new(config),
            generation: std::sync::atomic::AtomicU64::new(0),
            encoding: std::sync::atomic::AtomicU8::new(1), // 0=utf8, 1=utf16
            show_document_supported: std::sync::atomic::AtomicBool::new(false),
            code_lens_refresh_supported: std::sync::atomic::AtomicBool::new(false),
            file_watch_dynamic_registration: std::sync::atomic::AtomicBool::new(false),
            work_done_progress_supported: std::sync::atomic::AtomicBool::new(false),
            can_create_files: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn bump_generation(&self) -> u64 {
        let next = self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.link_tx.send(next);
        next
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get (or create) the per-URI mutex for serializing document notifications.
    pub fn doc_lock(&self, uri: &Uri) -> Arc<Mutex<()>> {
        self.doc_locks
            .entry(uri.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub fn set_encoding(&self, enc: crate::offset::Encoding) {
        let v = match enc {
            crate::offset::Encoding::Utf8 => 0,
            crate::offset::Encoding::Utf16 => 1,
        };
        self.encoding.store(v, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_encoding(&self) -> crate::offset::Encoding {
        match self.encoding.load(std::sync::atomic::Ordering::Relaxed) {
            0 => crate::offset::Encoding::Utf8,
            _ => crate::offset::Encoding::Utf16,
        }
    }
}

// ── Positions ─────────────────────────────────────────────────────────────────

pub fn range_from_node(node: tree_sitter::Node<'_>, src: &[u8], enc: crate::offset::Encoding) -> Range {
    Range {
        start: crate::offset::offset_to_position(src, node.start_byte(), enc),
        end: crate::offset::offset_to_position(src, node.end_byte(), enc),
    }
}
