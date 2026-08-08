import type { FileEntry } from "@/types/global";

export interface ResolvedLocalDropPathEntry {
  path: string;
  isDir: boolean;
}

export interface RemoteTextFile {
  path: string;
  content: string;
  size: number;
  mtime?: number;
}

export interface RemoteBinaryFile {
  path: string;
  contentBytes: number[] | Uint8Array | ArrayBuffer;
  size: number;
  mtime?: number;
}

export type FileExplorerSessionCache = {
  files: FileEntry[];
  currentPath: string;
  homeDir: string;
  history: string[];
  historyIndex: number;
  visitedHistory: string[];
};

export type LoadDirectoryOptions = {
  history?: "push" | "preserve";
  selectEntryName?: string;
  rawPathToken?: string;
};

export type FileExplorerBackendKind = "remote" | "local";

export type BreadcrumbSegment = {
  id: string;
  label: string;
  path: string;
  isRoot: boolean;
  isCurrent: boolean;
};

export type DirectoryChild = {
  name: string;
  path: string;
  isSymlink: boolean;
  rawPathToken?: string;
};

export type ChildrenMenuState =
  | { status: "idle" }
  | { status: "loading"; path: string }
  | { status: "success"; path: string; items: DirectoryChild[] }
  | { status: "error"; path: string; message: string };

export type InlineRenameState = {
  entryName: string;
  oldPath: string;
  oldRawPathToken?: string;
  initialName: string;
  value: string;
  isSubmitting: boolean;
};

export const fileExplorerSessionCacheStore = new Map<string, FileExplorerSessionCache>();
export const MAX_VISITED_HISTORY = 30;
export const FILE_LIST_ITEM_HEIGHT = 30;
export const FILE_LIST_HEADER_HEIGHT = 28;
export const FILE_LIST_OVERSCAN = 8;
export const PARENT_DIRECTORY_ENTRY_NAME = "..";

export type FileSortColumn = "name" | "mtime" | "size" | "permissions" | "owner" | "group";
export type FileSortDirection = "asc" | "desc";
export type FileSortMode = {
  column: FileSortColumn;
  direction: FileSortDirection;
};
export type FileListColumn = {
  id: FileSortColumn;
  labelKey: string;
  align?: "left" | "right";
};
export type FileListColumnWidths = Record<FileSortColumn, number>;

export const FILE_LIST_COLUMNS: FileListColumn[] = [
  { id: "name", labelKey: "fileExplorer.name" },
  { id: "mtime", labelKey: "fileExplorer.mtime" },
  { id: "size", labelKey: "fileExplorer.size", align: "right" },
  { id: "permissions", labelKey: "fileExplorer.permissions" },
  { id: "owner", labelKey: "fileExplorer.owner" },
  { id: "group", labelKey: "fileExplorer.group" },
];

export const DEFAULT_FILE_LIST_COLUMN_WIDTHS: FileListColumnWidths = {
  name: 220,
  mtime: 128,
  size: 80,
  permissions: 112,
  owner: 96,
  group: 96,
};

export const MIN_FILE_LIST_COLUMN_WIDTHS: FileListColumnWidths = {
  name: 140,
  mtime: 112,
  size: 72,
  permissions: 92,
  owner: 76,
  group: 76,
};

export const DEFAULT_FILE_SORT_DIRECTIONS: Record<FileSortColumn, FileSortDirection> = {
  name: "asc",
  mtime: "desc",
  size: "desc",
  permissions: "asc",
  owner: "asc",
  group: "asc",
};

export const PARENT_DIRECTORY_ENTRY: FileEntry = {
  name: PARENT_DIRECTORY_ENTRY_NAME,
  is_dir: true,
  is_symlink: false,
  size: 0,
  permissions: "",
  owner: "",
  group: "",
  mtime: 0,
};

export const BINARY_EXTENSIONS = new Set([
  "jpg",
  "jpeg",
  "png",
  "gif",
  "bmp",
  "webp",
  "ico",
  "tiff",
  "tif",
  "heic",
  "heif",
  "avif",
  "jfif",
  "psd",
  "ai",
  "eps",
  "raw",
  "cr2",
  "nef",
  "mp3",
  "wav",
  "flac",
  "aac",
  "ogg",
  "wma",
  "m4a",
  "aiff",
  "opus",
  "mp4",
  "avi",
  "mkv",
  "mov",
  "wmv",
  "flv",
  "webm",
  "m4v",
  "3gp",
  "mpeg",
  "mpg",
  "zip",
  "rar",
  "7z",
  "tar",
  "gz",
  "bz2",
  "xz",
  "lz",
  "lzma",
  "zst",
  "tgz",
  "tbz2",
  "txz",
  "cab",
  "iso",
  "dmg",
  "exe",
  "dll",
  "so",
  "dylib",
  "bin",
  "app",
  "msi",
  "deb",
  "rpm",
  "apk",
  "ipa",
  "jar",
  "war",
  "ear",
  "pdf",
  "doc",
  "docx",
  "xls",
  "xlsx",
  "ppt",
  "pptx",
  "odt",
  "ods",
  "odp",
  "ttf",
  "otf",
  "woff",
  "woff2",
  "eot",
  "db",
  "sqlite",
  "sqlite3",
  "mdb",
  "accdb",
  "o",
  "obj",
  "pyc",
  "pyo",
  "class",
  "beam",
  "swf",
  "fla",
  "blend",
  "unity3d",
  "unitypackage",
]);

const LANGUAGE_BY_EXTENSION: Record<string, string> = {
  bat: "batch",
  bash: "shell",
  c: "c",
  cfg: "ini",
  cc: "cpp",
  cjs: "javascript",
  cmd: "batch",
  conf: "ini",
  cpp: "cpp",
  cs: "csharp",
  css: "css",
  cxx: "cpp",
  dart: "dart",
  diff: "diff",
  dockerfile: "dockerfile",
  fs: "fsharp",
  fish: "shell",
  gql: "graphql",
  graphql: "graphql",
  go: "go",
  h: "c",
  hpp: "cpp",
  htm: "html",
  html: "html",
  ini: "ini",
  just: "makefile",
  java: "java",
  js: "javascript",
  json: "json",
  json5: "json5",
  jsonc: "jsonc",
  jsx: "javascript",
  kt: "kotlin",
  kts: "kotlin",
  less: "less",
  log: "plaintext",
  lua: "lua",
  markdown: "markdown",
  md: "markdown",
  mdx: "markdown",
  mjs: "javascript",
  patch: "diff",
  pl: "perl",
  php: "php",
  properties: "properties",
  proto: "protobuf",
  ps1: "powershell",
  psm1: "powershell",
  py: "python",
  pyi: "python",
  pyw: "python",
  r: "r",
  R: "r",
  rb: "ruby",
  rs: "rust",
  sass: "sass",
  scss: "scss",
  sh: "shell",
  sql: "sql",
  svelte: "svelte",
  swift: "swift",
  toml: "toml",
  ts: "typescript",
  tsx: "typescript",
  txt: "plaintext",
  vb: "vb",
  vue: "vue",
  xml: "xml",
  xhtml: "html",
  xsl: "xml",
  xslt: "xml",
  yaml: "yaml",
  yml: "yaml",
  zsh: "shell",
};

/** Maximum bytes the internal editor / text preview will read. */
export const TEXT_PREVIEW_MAX_BYTES = 5 * 1024 * 1024;
/** Maximum bytes for CSV/TSV tabular previews. */
export const CSV_PREVIEW_MAX_BYTES = 10 * 1024 * 1024;

const SHELL_TEXT_BASENAMES = new Set([
  "bash_profile",
  "bash_login",
  "bash_logout",
  "bashrc",
  "profile",
  "zprofile",
  "zshenv",
  "zshrc",
  "zlogin",
  "zlogout",
  "kshrc",
  "cshrc",
  "tcshrc",
]);

const CONFIG_TEXT_BASENAMES = new Set([
  "env",
  "env.local",
  "env.development",
  "env.production",
  "env.test",
  "gitconfig",
  "editorconfig",
  "npmrc",
  "yarnrc",
  "curlrc",
  "wgetrc",
]);

export function getLocalPathName(path: string, fallback: string) {
  return path.split(/[\\/]/).pop() || fallback;
}

export function getFileExtension(name: string) {
  const normalized = name.trim().toLocaleLowerCase();
  const lastSlash = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  const baseName = normalized.slice(lastSlash + 1);
  const index = baseName.lastIndexOf(".");
  return index > 0 ? baseName.slice(index + 1) : "";
}

export type FilePreviewKind = "image" | "markdown" | "csv" | "json" | "text" | "pdf";

export const IMAGE_PREVIEW_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
export const MARKDOWN_PREVIEW_EXTENSIONS = new Set(["md", "markdown", "mdx"]);
export const CSV_PREVIEW_EXTENSIONS = new Set(["csv", "tsv"]);
export const JSON_PREVIEW_EXTENSIONS = new Set(["json", "jsonc", "json5"]);

/**
 * Chooses how a file is rendered. Only image/pdf need a binary read; every
 * other file is rendered as text. Whether a file is actually readable as
 * text is decided by the backend read (`read_*_file_text`), never by
 * filename heuristics here.
 */
export function getFilePreviewKind(name: string): FilePreviewKind {
  const ext = getFileExtension(name);
  if (IMAGE_PREVIEW_EXTENSIONS.has(ext)) return "image";
  if (MARKDOWN_PREVIEW_EXTENSIONS.has(ext)) return "markdown";
  if (CSV_PREVIEW_EXTENSIONS.has(ext)) return "csv";
  if (JSON_PREVIEW_EXTENSIONS.has(ext)) return "json";
  if (ext === "pdf") return "pdf";
  return "text";
}

export function imageMimeFromFilename(name: string) {
  switch (getFileExtension(name)) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "bmp":
      return "image/bmp";
    default:
      return "application/octet-stream";
  }
}

export function isKnownBinaryFile(name: string) {
  const ext = getFileExtension(name);
  return !!ext && BINARY_EXTENSIONS.has(ext);
}

function getNormalizedBaseName(name: string) {
  return name.split(/[\\/]/).pop()?.toLocaleLowerCase() || name.toLocaleLowerCase();
}

export function languageFromFilename(name: string) {
  const baseName = getNormalizedBaseName(name);
  const normalized = baseName.replace(/^\.+/, "");

  if (SHELL_TEXT_BASENAMES.has(normalized)) {
    return "shell";
  }

  if (CONFIG_TEXT_BASENAMES.has(normalized)) {
    return "ini";
  }

  if (baseName === "cmakelists.txt") return "cmake";
  if (baseName === "dockerfile" || baseName.endsWith(".dockerfile")) return "dockerfile";
  if (baseName === "makefile" || baseName === "gnumakefile" || baseName === "justfile") {
    return "makefile";
  }
  if (baseName === "nginx.conf" || baseName.endsWith(".nginx.conf")) return "nginx";
  if (baseName === "docker-compose.yml" || baseName === "docker-compose.yaml") return "yaml";
  return LANGUAGE_BY_EXTENSION[getFileExtension(name)] || "plaintext";
}

export function buildRemoteUploadPath(remoteDir: string, name: string) {
  return remoteDir === "/" ? `/${name}` : `${remoteDir}/${name}`;
}

export function normalizeDirectoryPath(path: string) {
  if (!path || path === "/") return path;
  const normalized = path.replace(/\/+$/, "");
  return normalized || "/";
}

function isWindowsDriveRoot(path: string) {
  return /^[a-zA-Z]:[\\/]?$/.test(path);
}

function normalizeWindowsDriveRoot(path: string) {
  return isWindowsDriveRoot(path) ? `${path.slice(0, 2)}\\` : path;
}

function isUncRoot(path: string) {
  const normalized = path.replace(/\//g, "\\");
  if (!normalized.startsWith("\\\\")) return false;
  const parts = normalized.split("\\").filter(Boolean);
  return parts.length <= 2;
}

function getLocalSeparator(path: string) {
  return path.includes("\\") || isWindowsDriveRoot(path) || path.startsWith("\\\\") ? "\\" : "/";
}

export function normalizeExplorerPath(path: string, backend: FileExplorerBackendKind) {
  if (backend === "remote") return normalizeDirectoryPath(path);
  const trimmed = path.trim();
  if (!trimmed) return "";
  if (trimmed === "/" || trimmed === "\\") return trimmed;
  if (isWindowsDriveRoot(trimmed)) return normalizeWindowsDriveRoot(trimmed);
  if (isUncRoot(trimmed)) return trimmed.replace(/[\\/]+$/, "\\");
  const normalized = trimmed.replace(/[\\/]+$/, "");
  return normalized || trimmed;
}

export function getRemoteParentDirectory(path: string) {
  const normalized = normalizeDirectoryPath(path);
  if (!normalized || normalized === "/") return "/";
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "/" : normalized.slice(0, index);
}

export function getExplorerParentDirectory(path: string, backend: FileExplorerBackendKind) {
  if (backend === "remote") return getRemoteParentDirectory(path);

  const normalized = normalizeExplorerPath(path, backend);
  if (!normalized || normalized === "/" || normalized === "\\" || isWindowsDriveRoot(normalized)) {
    return normalized;
  }
  if (isUncRoot(normalized)) return normalized;

  const lastSlash = Math.max(normalized.lastIndexOf("/"), normalized.lastIndexOf("\\"));
  if (lastSlash < 0) return normalized;
  if (lastSlash === 0) return normalized.slice(0, 1);
  if (lastSlash === 2 && /^[a-zA-Z]:/.test(normalized)) return `${normalized.slice(0, 2)}\\`;
  return normalized.slice(0, lastSlash);
}

export function joinExplorerPath(basePath: string, name: string, backend: FileExplorerBackendKind) {
  if (backend === "remote") {
    return basePath === "/" ? `/${name}` : `${basePath}/${name}`;
  }
  const normalizedBase = normalizeExplorerPath(basePath, backend);
  const separator = getLocalSeparator(normalizedBase);
  if (!normalizedBase) return name;
  if (normalizedBase.endsWith("/") || normalizedBase.endsWith("\\")) {
    return `${normalizedBase}${name}`;
  }
  return `${normalizedBase}${separator}${name}`;
}

export function pathStartsWithDirectory(
  path: string,
  directory: string,
  backend: FileExplorerBackendKind,
) {
  const normalizedPath = normalizeExplorerPath(path, backend);
  const normalizedDirectory = normalizeExplorerPath(directory, backend);
  if (!normalizedPath || !normalizedDirectory) return false;
  if (normalizedPath === normalizedDirectory) return true;
  const separator = getLocalSeparator(normalizedDirectory);
  const prefix =
    normalizedDirectory.endsWith("/") || normalizedDirectory.endsWith("\\")
      ? normalizedDirectory
      : `${normalizedDirectory}${backend === "remote" ? "/" : separator}`;
  if (backend === "remote") return normalizedPath.startsWith(prefix);

  const isWindowsStylePath =
    /^[a-zA-Z]:[\\/]/.test(normalizedPath) ||
    /^[a-zA-Z]:[\\/]/.test(normalizedDirectory) ||
    normalizedPath.startsWith("\\\\") ||
    normalizedDirectory.startsWith("\\\\") ||
    normalizedPath.includes("\\") ||
    normalizedDirectory.includes("\\");
  return isWindowsStylePath
    ? normalizedPath.toLocaleLowerCase().startsWith(prefix.toLocaleLowerCase())
    : normalizedPath.startsWith(prefix);
}

export function buildBreadcrumbSegments(
  currentPath: string,
  homeDir: string,
  backend: FileExplorerBackendKind,
): BreadcrumbSegment[] {
  const normalizedPath = normalizeExplorerPath(currentPath || homeDir, backend);
  if (!normalizedPath) return [];

  const makeSegment = (label: string, path: string, isRoot: boolean): BreadcrumbSegment => {
    const normalizedSegmentPath = normalizeExplorerPath(path, backend);
    return {
      id: normalizedSegmentPath || path || label,
      label,
      path: normalizedSegmentPath || path,
      isRoot,
      isCurrent: normalizedSegmentPath === normalizedPath,
    };
  };

  if (backend === "remote") {
    const rootPath = "/";
    const segments = [makeSegment("/", rootPath, true)];
    const suffix = normalizedPath === rootPath ? "" : normalizedPath.slice(rootPath.length);
    const parts = suffix.split("/").filter(Boolean);
    let accumulated = rootPath;
    for (const part of parts) {
      accumulated = joinExplorerPath(accumulated, part, backend);
      segments.push(makeSegment(part, accumulated, false));
    }
    return segments;
  }

  const normalized = normalizedPath.replace(/\//g, "\\");
  if (normalized.startsWith("\\\\")) {
    const parts = normalized.split("\\").filter(Boolean);
    if (parts.length >= 2) {
      const rootPath = `\\\\${parts[0]}\\${parts[1]}`;
      const segments = [makeSegment(rootPath, rootPath, true)];
      let accumulated = rootPath;
      for (const part of parts.slice(2)) {
        accumulated = joinExplorerPath(accumulated, part, backend);
        segments.push(makeSegment(part, accumulated, false));
      }
      return segments;
    }
    return [makeSegment(normalizedPath, normalizedPath, true)];
  }

  const driveMatch = normalizedPath.match(/^([a-zA-Z]:)[\\/]?/);
  if (driveMatch) {
    const rootPath = `${driveMatch[1]}\\`;
    const segments = [makeSegment(driveMatch[1], rootPath, true)];
    const suffix = normalizedPath.slice(driveMatch[0].length);
    const parts = suffix.split(/[\\/]/).filter(Boolean);
    let accumulated = rootPath;
    for (const part of parts) {
      accumulated = joinExplorerPath(accumulated, part, backend);
      segments.push(makeSegment(part, accumulated, false));
    }
    return segments;
  }

  if (normalizedPath.startsWith("/") || normalizedPath.startsWith("\\")) {
    const rootPath = normalizedPath.slice(0, 1);
    const segments = [makeSegment(rootPath, rootPath, true)];
    const parts = normalizedPath.slice(1).split(/[\\/]/).filter(Boolean);
    let accumulated = rootPath;
    for (const part of parts) {
      accumulated = joinExplorerPath(accumulated, part, backend);
      segments.push(makeSegment(part, accumulated, false));
    }
    return segments;
  }

  const parts = normalizedPath.split(/[\\/]/).filter(Boolean);
  let accumulated = "";
  return parts.map((part, index) => {
    accumulated = accumulated ? joinExplorerPath(accumulated, part, backend) : part;
    return makeSegment(part, accumulated, index === 0);
  });
}

export function isParentDirectoryEntry(entry: FileEntry) {
  return entry.name === PARENT_DIRECTORY_ENTRY_NAME;
}

/**
 * Records a freshly visited directory into the in-memory visited list.
 * Deduplicates by path (moving an existing entry to the top), keeps the most
 * recent first, and caps the list to avoid unbounded growth per session.
 */
export function pushVisitedHistory(
  list: string[],
  path: string,
  backend: FileExplorerBackendKind = "remote",
): string[] {
  const normalizedPath = normalizeExplorerPath(path, backend);
  if (!normalizedPath) return list;
  const withoutDuplicate = list.filter((entry) => entry !== normalizedPath);
  withoutDuplicate.unshift(normalizedPath);
  if (withoutDuplicate.length > MAX_VISITED_HISTORY) {
    withoutDuplicate.length = MAX_VISITED_HISTORY;
  }
  return withoutDuplicate;
}

function naturalCompare(left: string, right: string) {
  return left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
}

export function compareFileEntries(left: FileEntry, right: FileEntry, sortMode: FileSortMode) {
  if (left.is_dir !== right.is_dir) return left.is_dir ? -1 : 1;

  let result = 0;
  switch (sortMode.column) {
    case "mtime":
      result = left.mtime - right.mtime;
      break;
    case "size":
      result = left.size - right.size;
      break;
    case "permissions":
      result = naturalCompare(left.permissions || "", right.permissions || "");
      break;
    case "owner":
      result = naturalCompare(left.owner || "", right.owner || "");
      break;
    case "group":
      result = naturalCompare(left.group || "", right.group || "");
      break;
    case "name":
      result = naturalCompare(left.name, right.name);
      break;
  }

  if (result !== 0) {
    return sortMode.direction === "asc" ? result : -result;
  }

  return naturalCompare(left.name, right.name);
}

export function matchesFileSearch(entry: FileEntry, query: string) {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return true;
  return entry.name.toLocaleLowerCase().includes(normalizedQuery);
}

export function buildSessionCacheSnapshot(
  files: FileEntry[],
  currentPath: string,
  homeDir: string,
  history: string[],
  historyIndex: number,
  visitedHistory: string[],
  backend: FileExplorerBackendKind = "remote",
): FileExplorerSessionCache | null {
  const normalizedCurrentPath = normalizeExplorerPath(currentPath, backend);
  const normalizedHomeDir = normalizeExplorerPath(homeDir, backend);
  const normalizedHistory = history
    .map((entry) => normalizeExplorerPath(entry, backend))
    .filter((entry): entry is string => !!entry);

  if (!normalizedCurrentPath) {
    return null;
  }

  const nextHistory = normalizedHistory.length > 0 ? normalizedHistory : [normalizedCurrentPath];
  const nextHistoryIndex = Math.min(Math.max(historyIndex, 0), nextHistory.length - 1);

  const normalizedVisited = visitedHistory
    .map((entry) => normalizeExplorerPath(entry, backend))
    .filter((entry): entry is string => !!entry)
    .slice(0, MAX_VISITED_HISTORY);

  return {
    files,
    currentPath: normalizedCurrentPath,
    homeDir: normalizedHomeDir || normalizedCurrentPath,
    history: nextHistory,
    historyIndex: nextHistoryIndex,
    visitedHistory: normalizedVisited,
  };
}
