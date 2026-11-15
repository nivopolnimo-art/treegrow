use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    Image,
    Video,
    Audio,
    Other,
}

#[derive(Clone)]
pub struct Node {
    pub parent: Option<usize>,
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub mtime: u64,
    pub depth: u32,
    pub children: u32,
}

pub fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "heic" | "tif" | "tiff" => Kind::Image,
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => Kind::Video,
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "flac" | "aac" | "wma" | "amr" => Kind::Audio,
        _ => Kind::Other,
    }
}

pub fn is_media(k: Kind) -> bool {
    matches!(k, Kind::Image | Kind::Video | Kind::Audio)
}

#[derive(Clone)]
pub struct ScanOpts {
    pub roots: Vec<PathBuf>,
    pub only_media: bool,
    pub skip_junk: bool,
    pub max_nodes: usize,
}

pub struct ScanShared {
    pub count: AtomicUsize,
    pub done: AtomicBool,
    pub cancel: AtomicBool,
    pub result: Mutex<Option<Vec<Node>>>,
}

impl ScanShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            count: AtomicUsize::new(0),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
        })
    }
}

fn keep(e: &walkdir::DirEntry, o: &ScanOpts) -> bool {
    if e.depth() == 0 || !o.skip_junk || !e.file_type().is_dir() {
        return true;
    }
    const JUNK: [&str; 9] = [
        "node_modules",
        "$RECYCLE.BIN",
        "System Volume Information",
        "Windows",
        "AppData",
        "$WinREAgent",
        "target",
        "__pycache__",
        "Recovery",
    ];
    let name = e.file_name().to_string_lossy();
    if name.starts_with('.') {
        return false;
    }
    !JUNK.iter().any(|j| name.eq_ignore_ascii_case(j))
}

/// Обход файлов. Дерево хранится в порядке обхода в глубину,
/// поэтому индекс родителя всегда меньше индекса ребёнка.
pub fn scan(o: &ScanOpts, count: &AtomicUsize, cancel: &AtomicBool) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let virt = o.roots.len() > 1;
    if virt {
        nodes.push(Node {
            parent: None,
            name: "Мой компьютер".into(),
            path: PathBuf::new(),
            kind: Kind::Dir,
            mtime: 0,
            depth: 0,
            children: 0,
        });
    }
    let per = (o.max_nodes / o.roots.len().max(1)).max(1);

    for root in &o.roots {
        let start = nodes.len();
        let mut stack: Vec<usize> = Vec::new();
        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| keep(e, o));
        for entry in walker {
            if cancel.load(Ordering::Relaxed) || nodes.len() - start >= per {
                break;
            }
            let Ok(entry) = entry else { continue };
            let d = entry.depth();
            let is_dir = entry.file_type().is_dir();
            let kind = if is_dir { Kind::Dir } else { kind_of(entry.path()) };
            if o.only_media && kind == Kind::Other {
                continue;
            }
            let parent = if d == 0 {
                if virt {
                    Some(0)
                } else {
                    None
                }
            } else {
                match stack.get(d - 1) {
                    Some(&p) => Some(p),
                    None => continue,
                }
            };
            let name = if d == 0 {
                root.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.to_string_lossy().into_owned())
            } else {
                entry.file_name().to_string_lossy().into_owned()
            };
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let id = nodes.len();
            if let Some(p) = parent {
                nodes[p].children += 1;
            }
            nodes.push(Node {
                parent,
                name,
                path: entry.path().to_path_buf(),
                kind,
                mtime,
                depth: d as u32 + virt as u32,
                children: 0,
            });
            if is_dir {
                stack.truncate(d);
                stack.push(id);
            }
            if id % 256 == 0 {
                count.store(nodes.len(), Ordering::Relaxed);
            }
        }
    }
    count.store(nodes.len(), Ordering::Relaxed);
    nodes
}

/// Порядок появления узлов. chrono=false: рост от корня кольцами.
/// chrono=true: по времени изменения файлов (родитель всегда раньше детей).
pub fn birth_order(nodes: &[Node], chrono: bool) -> Vec<usize> {
    let n = nodes.len();
    let mut ids: Vec<usize> = (0..n).collect();
    if chrono {
        let mut eff = vec![0u64; n];
        for i in 0..n {
            let m = nodes[i].mtime;
            eff[i] = match nodes[i].parent {
                Some(p) => m.max(eff[p]),
                None => m,
            };
        }
        ids.sort_by_key(|&i| (eff[i], nodes[i].depth, i));
    } else {
        ids.sort_by_key(|&i| (nodes[i].depth, i));
    }
    ids
}
