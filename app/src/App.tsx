import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

interface ScanNode {
  name: string;
  path: string;
  size: number;
  files: number;
  dirs: number;
  errors: number;
  largest: FileEntry[];
  oldest: FileEntry[];
  children: ScanNode[];
}

interface FileEntry {
  name: string;
  path: string;
  size: number;
  modified: number;
}

interface DupGroup {
  hash: string;
  size: number;
  files: string[];
}

interface DupReport {
  groups: DupGroup[];
  errors: number;
}

interface TrashItem {
  path: string;
  ok: boolean;
  error: string;
}

interface TrashReport {
  results: TrashItem[];
  trashed: number;
  failed: number;
}

interface Roots {
  roots: string[];
  home: string;
  temp: string;
}

interface OrphanEntry {
  name: string;
  path: string;
  size: number;
}

interface OrphanReport {
  orphans: OrphanEntry[];
  totalBytes: number;
  scanned: number;
  referenced: number;
}

interface RemnantEntry {
  name: string;
  path: string;
  size: number;
}

interface RemnantReport {
  emptyDirs: RemnantEntry[];
  staleFiles: RemnantEntry[];
  errors: number;
}

type View = "care" | "disk" | "clutter" | "orphans" | "sweep" | "about";

function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let v = n / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[u]}`;
}

function fmtInt(n: number): string {
  return n.toLocaleString("en-US");
}

function fmtDate(secs: number): string {
  if (!secs || secs < 0) return "—";
  const d = new Date(secs * 1000);
  return Number.isNaN(d.getTime()) ? "—" : d.toLocaleDateString();
}

function errText(e: unknown): string {
  return typeof e === "string" ? e : String(e);
}

function hue(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % 360;
}

const REPO = "https://github.com/yourbossdonpely/oscleaner";

const THEMES: Record<View, string> = {
  care: "theme-care",
  disk: "theme-disk",
  clutter: "theme-clutter",
  orphans: "theme-orphans",
  sweep: "theme-sweep",
  about: "theme-about",
};

function Titlebar() {
  const win = getCurrentWindow();
  return (
    <div className="titlebar" data-tauri-drag-region>
      <img src="/icon.png" alt="" draggable={false} />
      <span>OS Cleaner</span>
      <div className="winbtns">
        <button onClick={() => win.minimize().catch(() => {})} aria-label="Minimize">
          —
        </button>
        <button onClick={() => win.toggleMaximize().catch(() => {})} aria-label="Maximize">
          ⛶
        </button>
        <button className="x" onClick={() => win.close().catch(() => {})} aria-label="Close">
          ✕
        </button>
      </div>
    </div>
  );
}

export default function App() {
  const [view, setView] = useState<View>("care");
  const [dark, setDark] = useState<boolean>(() => {
    try {
      return localStorage.getItem("oc-theme") === "dark";
    } catch {
      return false;
    }
  });
  function toggleDark() {
    setDark((d) => {
      const n = !d;
      try {
        localStorage.setItem("oc-theme", n ? "dark" : "light");
      } catch {
        /* private mode: don't persist */
      }
      return n;
    });
  }
  return (
    <div className={`app ${THEMES[view]}${dark ? " dark" : ""}`}>
      <Titlebar />
      <div className="body">
      <aside className="side">
        <div className="brand">
          <img src="/icon.png" alt="OS Cleaner" />
          <div>
            <div className="bt">OS Cleaner</div>
            <div className="bs">BY DON PELY</div>
          </div>
        </div>
        <nav className="nav">
          <div className="cap">CARE</div>
          <NavBtn id="care" view={view} setView={setView} label="Smart Care" icon="✦" />
          <div className="cap">INSIGHT</div>
          <NavBtn id="disk" view={view} setView={setView} label="Disk Insight" icon="▦" />
          <NavBtn id="clutter" view={view} setView={setView} label="My Clutter" icon="⧉" />
          <div className="cap">MODULES</div>
          <NavBtn id="orphans" view={view} setView={setView} label="Installer Orphans" icon="▤" />
          <NavBtn id="sweep" view={view} setView={setView} label="Remnant Sweep" icon="❖" />
          <button className="navbtn" disabled title="Coming in v0.3">
            <span className="ni">◧</span>App Leftovers<span className="soon">soon</span>
          </button>
          <div className="cap">APP</div>
          <NavBtn id="about" view={view} setView={setView} label="About" icon="ⓘ" />
        </nav>
        <div className="sidefoot">
          <span>v0.4.0 · cleaner modules</span>
          <button className="themebtn" onClick={toggleDark} title="Toggle dark mode">
            {dark ? "☀" : "◐"}
          </button>
        </div>
      </aside>
      <main className="main">
        {view === "care" && <SmartCare />}
        {view === "disk" && <DiskInsight />}
        {view === "clutter" && <MyClutter />}
        {view === "orphans" && <OrphansScreen />}
        {view === "sweep" && <SweepScreen />}
        {view === "about" && <About />}
      </main>
      </div>
    </div>
  );
}

function NavBtn(props: {
  id: View;
  view: View;
  setView: (v: View) => void;
  label: string;
  icon: string;
}) {
  return (
    <button
      className={"navbtn" + (props.view === props.id ? " active" : "")}
      onClick={() => props.setView(props.id)}
    >
      <span className="ni">{props.icon}</span>
      {props.label}
    </button>
  );
}

function useRoots(def: (r: Roots) => string) {
  const [roots, setRoots] = useState<Roots>({ roots: [], home: "", temp: "" });
  const [path, setPath] = useState("");
  useEffect(() => {
    invoke<Roots>("list_roots")
      .then((r) => {
        setRoots(r);
        setPath((p) => p || def(r));
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return { roots, path, setPath };
}

function RootPills(props: { roots: Roots; path: string; setPath: (p: string) => void }) {
  const all = props.roots.home
    ? [...props.roots.roots, props.roots.home]
    : props.roots.roots;
  if (all.length === 0) return null;
  return (
    <div className="roots">
      {all.map((r) => (
        <button
          key={r}
          className={"pill" + (props.path === r ? " on" : "")}
          onClick={() => props.setPath(r)}
          title={r}
        >
          {r}
        </button>
      ))}
    </div>
  );
}

function Stat(props: { label: string; value: string }) {
  return (
    <div className="stat">
      <div className="sv">{props.value}</div>
      <div className="sl">{props.label}</div>
    </div>
  );
}

function SmartCare() {
  const { roots, path, setPath } = useRoots((r) => r.home || r.roots[0] || "");
  const [scan, setScan] = useState<ScanNode | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function runScan() {
    if (!path || busy) return;
    setBusy(true);
    setError("");
    setScan(null);
    try {
      const node = await invoke<ScanNode>("scan_dir", { path });
      setScan(node);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  const top = scan
    ? [...scan.children].sort((a, b) => b.size - a.size).slice(0, 8)
    : [];
  const maxTop = top.length > 0 ? top[0].size : 1;

  return (
    <div>
      <div className="heroband">
        <img src="/icon.png" alt="" className="heroicon" draggable={false} />
        <div className="herotext">
          <h1>Smart Care</h1>
          <p>One scan for everything — junk, clutter, and space hogs.</p>
        </div>
        <span className="vtag">v0.4</span>
      </div>
      <div className="card hero">
        <div className="scanrow">
          <input
            className="pathinput"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Folder to scan…"
            spellCheck={false}
          />
          <button
            className="btn primary"
            onClick={runScan}
            disabled={busy || !path}
          >
            {busy ? "Scanning…" : "Scan"}
          </button>
        </div>
        <RootPills roots={roots} path={path} setPath={setPath} />
        {busy && (
          <div className="loading">
            <span className="spin" />
            Walking files{path ? ` under ${path}` : ""}…
          </div>
        )}
        {error !== "" && <div className="err">{error}</div>}
      </div>
      {scan && (
        <>
          <div className="stats">
            <Stat label="Total size" value={fmtBytes(scan.size)} />
            <Stat label="Files" value={fmtInt(scan.files)} />
            <Stat label="Folders" value={fmtInt(scan.dirs)} />
            <Stat label="Unreadable" value={fmtInt(scan.errors)} />
          </div>
          <div className="card">
            <h2>Largest folders</h2>
            {top.length === 0 && (
              <p className="muted">No subfolders found.</p>
            )}
            {top.map((t) => (
              <div key={t.path} className="toprow">
                <span className="topname" title={t.path}>
                  {t.name}
                </span>
                <span className="topbar-track">
                  <i
                    style={{
                      width: `${Math.max(2, (t.size / maxTop) * 100)}%`,
                    }}
                  />
                </span>
                <span className="topsize">{fmtBytes(t.size)}</span>
              </div>
            ))}
          </div>
          <div className="card note">
            v0.2 scans and shows. Automatic cleanup with backup-first safety
            lands in v0.3 — <b>nothing here deletes anything.</b>
          </div>
        </>
      )}
    </div>
  );
}

interface RowState {
  node: ScanNode;
  depth: number;
  parent: number;
}

function DiskInsight() {
  const { roots, path, setPath } = useRoots((r) => r.roots[0] || r.home || "");
  const [root, setRoot] = useState<ScanNode | null>(null);
  const [expanded, setExpanded] = useState<Record<string, ScanNode[]>>({});
  const [loading, setLoading] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [tab, setTab] = useState<"table" | "map">("table");

  async function runScan() {
    if (!path || busy) return;
    setBusy(true);
    setError("");
    setRoot(null);
    setExpanded({});
    try {
      const node = await invoke<ScanNode>("scan_dir", { path });
      setRoot(node);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function toggle(node: ScanNode) {
    if (expanded[node.path]) {
      const c = { ...expanded };
      delete c[node.path];
      setExpanded(c);
      return;
    }
    if (node.children.length > 0) {
      setExpanded((e) => ({ ...e, [node.path]: node.children }));
      return;
    }
    if (node.dirs === 0) return;
    setLoading((l) => ({ ...l, [node.path]: true }));
    try {
      const full = await invoke<ScanNode>("scan_dir", { path: node.path });
      setExpanded((e) => ({ ...e, [node.path]: full.children }));
    } catch (e) {
      setError(errText(e));
    } finally {
      setLoading((l) => {
        const c = { ...l };
        delete c[node.path];
        return c;
      });
    }
  }

  function rows(): RowState[] {
    const out: RowState[] = [];
    function walk(nodes: ScanNode[], depth: number, parentSize: number) {
      for (const n of nodes) {
        out.push({ node: n, depth, parent: parentSize });
        const kids = expanded[n.path];
        if (kids) walk(kids, depth + 1, n.size);
      }
    }
    if (root) {
      out.push({ node: root, depth: 0, parent: root.size });
      walk(root.children, 1, root.size);
    }
    return out;
  }

  return (
    <div>
      <div className="topbar">
        <div>
          <h1>Disk Insight</h1>
          <p>Browse folders by real size — table or treemap.</p>
        </div>
        <span className="vtag">v0.4</span>
      </div>
      <div className="card hero">
        <div className="scanrow">
          <input
            className="pathinput"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Folder to scan…"
            spellCheck={false}
          />
          <button
            className="btn primary"
            onClick={runScan}
            disabled={busy || !path}
          >
            {busy ? "Scanning…" : "Scan"}
          </button>
        </div>
        <RootPills roots={roots} path={path} setPath={setPath} />
        {busy && (
          <div className="loading">
            <span className="spin" />
            Walking files{path ? ` under ${path}` : ""}…
          </div>
        )}
        {error !== "" && <div className="err">{error}</div>}
      </div>
      {root && (
        <>
          <div className="tabs">
            <button
              className={tab === "table" ? "tab on" : "tab"}
              onClick={() => setTab("table")}
            >
              Table
            </button>
            <button
              className={tab === "map" ? "tab on" : "tab"}
              onClick={() => setTab("map")}
            >
              Treemap
            </button>
          </div>
          {tab === "table" ? (
            <div className="card tablecard">
              <table className="dtable">
                <thead>
                  <tr>
                    <th>Name</th>
                    <th className="num">Size</th>
                    <th className="num">Files</th>
                    <th className="num">% of parent</th>
                  </tr>
                </thead>
                <tbody>
                  {rows().map(({ node, depth, parent }) => (
                    <tr key={node.path}>
                      <td>
                        <span style={{ paddingLeft: depth * 22 }}>
                          {node.dirs > 0 ? (
                            <button
                              className="twisty"
                              onClick={() => void toggle(node)}
                              aria-label="expand"
                            >
                              {depth === 0 || expanded[node.path] ? "▾" : "▸"}
                            </button>
                          ) : (
                            <span className="twisty" />
                          )}
                          <span className="fname" title={node.path}>
                            {loading[node.path] ? "… " : ""}
                            {node.name}
                          </span>
                        </span>
                      </td>
                      <td className="num">{fmtBytes(node.size)}</td>
                      <td className="num">{fmtInt(node.files)}</td>
                      <td className="num">
                        {parent > 0
                          ? `${((node.size / parent) * 100).toFixed(1)}%`
                          : "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <div className="card">
              <TreemapView key={root.path} root={root} />
            </div>
          )}
        </>
      )}
    </div>
  );
}

interface TmRect {
  node: ScanNode;
  x: number;
  y: number;
  w: number;
  h: number;
}

function treemapLayout(nodes: ScanNode[], W: number, H: number): TmRect[] {
  const total = nodes.reduce((s, n) => s + n.size, 0) || 1;
  const items = nodes
    .map((n) => ({ node: n, area: (n.size / total) * W * H }))
    .filter((i) => i.area > 0)
    .sort((a, b) => b.area - a.area);
  const rects: TmRect[] = [];
  let row: typeof items = [];
  let x = 0;
  let y = 0;
  let w = W;
  let h = H;
  const worst = (r: typeof items, side: number): number => {
    if (r.length === 0) return Infinity;
    const s = r.reduce((a, b) => a + b.area, 0);
    const mx = Math.max(...r.map((i) => i.area));
    const mn = Math.min(...r.map((i) => i.area));
    if (mn <= 0 || s <= 0) return Infinity;
    return Math.max((side * side * mx) / (s * s), (s * s) / (side * side * mn));
  };
  const layoutRow = () => {
    const s = row.reduce((a, b) => a + b.area, 0);
    if (s <= 0) {
      row = [];
      return;
    }
    if (w >= h) {
      const rh = s / w;
      let cx = x;
      for (const it of row) {
        const cw = it.area / rh;
        rects.push({ node: it.node, x: cx, y, w: cw, h: rh });
        cx += cw;
      }
      y += rh;
      h -= rh;
    } else {
      const cw = s / h;
      let cy = y;
      for (const it of row) {
        const rh = it.area / cw;
        rects.push({ node: it.node, x, y: cy, w: cw, h: rh });
        cy += rh;
      }
      x += cw;
      w -= cw;
    }
    row = [];
  };
  for (const it of items) {
    const side = Math.min(w, h);
    if (row.length > 0 && worst([...row, it], side) > worst(row, side)) {
      layoutRow();
    }
    row.push(it);
  }
  layoutRow();
  return rects;
}

function TreemapView({ root }: { root: ScanNode }) {
  const [trail, setTrail] = useState<ScanNode[]>([root]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const current = trail[trail.length - 1];

  async function drill(node: ScanNode) {
    if (node.dirs === 0 || busy) return;
    setBusy(true);
    setError("");
    try {
      const full = await invoke<ScanNode>("scan_dir", { path: node.path });
      setTrail((t) => [...t, full]);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  const rects = treemapLayout(current.children, 1000, 600);
  return (
    <div>
      {trail.length > 1 && (
        <div className="crumbs">
          {trail.map((t, i) => (
            <span key={t.path}>
              {i > 0 && <span className="sep">›</span>}
              {i < trail.length - 1 ? (
                <button
                  className="crumb"
                  onClick={() => setTrail(trail.slice(0, i + 1))}
                >
                  {t.name}
                </button>
              ) : (
                <b>{t.name}</b>
              )}
            </span>
          ))}
          {busy && <span className="muted"> loading…</span>}
        </div>
      )}
      {error !== "" && <div className="err">{error}</div>}
      {rects.length === 0 && (
        <p className="muted">Nothing to map at this level.</p>
      )}
      <svg className="treemap" viewBox="0 0 1000 600" role="img">
        {rects.map((r) => (
          <g key={r.node.path}>
            <title>{`${r.node.path} — ${fmtBytes(r.node.size)}`}</title>
            <rect
              x={r.x + 1}
              y={r.y + 1}
              width={Math.max(0, r.w - 2)}
              height={Math.max(0, r.h - 2)}
              rx={6}
              fill={`hsl(${hue(r.node.name)} 70% 60%)`}
              className={r.node.dirs > 0 ? "tmrect" : "tmrect leaf"}
              onClick={() => void drill(r.node)}
            />
            {r.w > 90 && r.h > 44 && (
              <text x={r.x + 10} y={r.y + 26} className="tmtitle">
                {r.node.name.slice(0, Math.max(4, Math.floor(r.w / 10)))}
              </text>
            )}
            {r.w > 90 && r.h > 66 && (
              <text x={r.x + 10} y={r.y + 46} className="tmsize">
                {fmtBytes(r.node.size)}
              </text>
            )}
          </g>
        ))}
      </svg>
    </div>
  );
}

function FileTable({
  files,
  showDate,
  selected,
  onToggle,
}: {
  files: FileEntry[];
  showDate: boolean;
  selected: Set<string>;
  onToggle: (path: string) => void;
}) {
  const shown = files.slice(0, 25);
  return (
    <div>
      <table className="dtable">
        <thead>
          <tr>
            <th className="checkcol" />
            <th>Name</th>
            <th className="num">Size</th>
            {showDate && <th className="num">Modified</th>}
          </tr>
        </thead>
        <tbody>
          {shown.map((f) => (
            <tr key={f.path} className={selected.has(f.path) ? "picked" : ""}>
              <td>
                <input
                  type="checkbox"
                  className="check"
                  checked={selected.has(f.path)}
                  onChange={() => onToggle(f.path)}
                  aria-label={`Select ${f.name}`}
                />
              </td>
              <td>
                <span className="fname" title={f.path}>
                  {f.name}
                </span>
              </td>
              <td className="num">{fmtBytes(f.size)}</td>
              {showDate && <td className="num">{fmtDate(f.modified)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
      {files.length > shown.length && (
        <p className="muted small">
          Showing {shown.length} of {files.length}.
        </p>
      )}
      {files.length === 0 && <p className="muted">No files found.</p>}
    </div>
  );
}

function ReviewBar(props: {
  count: number;
  bytes: number;
  cta: string;
  onReview: () => void;
  onClear: () => void;
}) {
  if (props.count === 0) return null;
  return (
    <div className="reviewbar">
      <span>
        <b>{props.count}</b> selected · {fmtBytes(props.bytes)}
      </span>
      <button className="btn primary" onClick={props.onReview}>
        {props.cta}
      </button>
      <button className="linkbtn" onClick={props.onClear}>
        Clear
      </button>
    </div>
  );
}

const ACTION_COPY = {
  trash: {
    title: "Move to trash?",
    blurb:
      "They will be moved to the recycle bin — recoverable, never permanently deleted.",
    confirm: (n: number) => `Move ${n} to trash`,
  },
  backup: {
    title: "Move to backup?",
    blurb:
      "They will be moved to the backup folder. Restore them any time below.",
    confirm: (n: number) => `Move ${n} to backup`,
  },
  restore: {
    title: "Restore from backup?",
    blurb: "They will be moved back to their original folder.",
    confirm: (n: number) => `Restore ${n}`,
  },
};

function ConfirmModal(props: {
  paths: string[];
  bytes: number;
  busy: boolean;
  report: TrashReport | null;
  action: "trash" | "backup" | "restore";
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <div className="overlay" onClick={props.busy ? undefined : props.onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        {props.report === null ? (
          <>
            <h2>{ACTION_COPY[props.action].title}</h2>
            <p className="muted">
              {props.paths.length} files ({fmtBytes(props.bytes)}).{" "}
              {ACTION_COPY[props.action].blurb}
            </p>
            <div className="modallist">
              {props.paths.map((p) => (
                <div key={p} className="modpath" title={p}>
                  {p}
                </div>
              ))}
            </div>
            <div className="modalbtns">
              <button className="btn" onClick={props.onClose} disabled={props.busy}>
                Cancel
              </button>
              <button
                className={props.action === "trash" ? "btn danger" : "btn primary"}
                onClick={props.onConfirm}
                disabled={props.busy}
              >
                {props.busy ? "Working…" : ACTION_COPY[props.action].confirm(props.paths.length)}
              </button>
            </div>
          </>
        ) : (
          <>
            <h2>
              {props.report.failed === 0 ? "Done" : "Finished with errors"}
            </h2>
            <p className="muted">
              {props.report.trashed} moved, {props.report.failed} failed.
            </p>
            <div className="modallist">
              {props.report.results.map((r) => (
                <div
                  key={r.path}
                  className={r.ok ? "modok" : "moderr"}
                  title={r.ok ? r.path : `${r.path} — ${r.error}`}
                >
                  {r.ok ? "✓ " : "✗ "}
                  {r.path}
                </div>
              ))}
            </div>
            <div className="modalbtns">
              <button className="btn primary" onClick={props.onClose}>
                Close
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function MyClutter() {
  const { roots, path, setPath } = useRoots((r) => r.home || r.roots[0] || "");
  const [scan, setScan] = useState<ScanNode | null>(null);
  const [dups, setDups] = useState<DupReport | null>(null);
  const [busy, setBusy] = useState<"scan" | "dups" | "trash" | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [report, setReport] = useState<TrashReport | null>(null);
  const [error, setError] = useState("");

  async function runScan() {
    if (!path || busy) return;
    setBusy("scan");
    setError("");
    setScan(null);
    setDups(null);
    try {
      const node = await invoke<ScanNode>("scan_dir", { path });
      setScan(node);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  }

  async function runDups() {
    if (!path || busy) return;
    setBusy("dups");
    setError("");
    setDups(null);
    try {
      const report = await invoke<DupReport>("find_duplicates", { path });
      setDups(report);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  }

  function toggle(path: string) {
    setSelected((s) => {
      const c = new Set(s);
      if (c.has(path)) c.delete(path);
      else c.add(path);
      return c;
    });
  }

  function sizeOf(path: string): number {
    for (const f of scan?.largest ?? []) if (f.path === path) return f.size;
    for (const f of scan?.oldest ?? []) if (f.path === path) return f.size;
    for (const g of dups?.groups ?? []) {
      if (g.files.includes(path)) return g.size;
    }
    return 0;
  }

  const selPaths = [...selected];
  const selBytes = selPaths.reduce((s, p) => s + sizeOf(p), 0);

  async function doTrash() {
    if (selPaths.length === 0 || busy) return;
    setBusy("trash");
    try {
      const rep = await invoke<TrashReport>("trash_delete", { paths: selPaths });
      setReport(rep);
      const gone = new Set(rep.results.filter((r) => r.ok).map((r) => r.path));
      if (gone.size > 0) {
        setScan((s) =>
          s === null
            ? s
            : {
                ...s,
                largest: s.largest.filter((f) => !gone.has(f.path)),
                oldest: s.oldest.filter((f) => !gone.has(f.path)),
              }
        );
        setDups((d) =>
          d === null
            ? d
            : {
                ...d,
                groups: d.groups
                  .map((g) => ({ ...g, files: g.files.filter((f) => !gone.has(f)) }))
                  .filter((g) => g.files.length > 1),
              }
        );
      }
    } catch (e) {
      setError(errText(e));
      setConfirming(false);
    } finally {
      setBusy(null);
    }
  }

  function closeModal() {
    setConfirming(false);
    setReport(null);
    setSelected(new Set());
  }

  const waste =
    dups?.groups.reduce((s, g) => s + g.size * (g.files.length - 1), 0) ?? 0;

  return (
    <div>
      <div className="topbar">
        <div>
          <h1>My Clutter</h1>
          <p>Large files, old files, and true duplicates by content hash.</p>
        </div>
        <span className="vtag">v0.4</span>
      </div>
      <div className="card hero">
        <div className="scanrow">
          <input
            className="pathinput"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Folder to scan…"
            spellCheck={false}
          />
          <button
            className="btn primary"
            onClick={runScan}
            disabled={busy !== null || !path}
          >
            {busy === "scan" ? "Scanning…" : "Scan"}
          </button>
          <button
            className="btn"
            onClick={runDups}
            disabled={busy !== null || !path}
            title="Hash-based duplicate detection (slower)"
          >
            {busy === "dups" ? "Hashing…" : "Find duplicates"}
          </button>
        </div>
        <RootPills roots={roots} path={path} setPath={setPath} />
        {busy !== null && (
          <div className="loading">
            <span className="spin" />
            {busy === "scan" ? "Walking files" : "Hashing candidate files"}
            {path ? ` under ${path}` : ""}…
          </div>
        )}
        {error !== "" && <div className="err">{error}</div>}
      </div>
      {scan && (
        <div className="cols">
          <div className="card">
            <h2>Largest files</h2>
            <FileTable files={scan.largest} showDate={false} selected={selected} onToggle={toggle} />
          </div>
          <div className="card">
            <h2>Oldest files</h2>
            <FileTable files={scan.oldest} showDate={true} selected={selected} onToggle={toggle} />
          </div>
        </div>
      )}
      {dups && (
        <div className="card">
          <h2>
            Duplicate files{" "}
            {dups.groups.length > 0 && (
              <span className="muted">· {fmtBytes(waste)} reclaimable</span>
            )}
          </h2>
          {dups.groups.length === 0 && (
            <p className="muted">No duplicates found.</p>
          )}
          {dups.groups.slice(0, 50).map((g) => (
            <div key={g.hash} className="dupgroup">
              <div className="duphead">
                {fmtBytes(g.size)} × {g.files.length}{" "}
                <span className="muted">({g.hash.slice(0, 12)}…)</span>
              </div>
              {g.files.map((f) => (
                <label key={f} className="dupfile" title={f}>
                  <input
                    type="checkbox"
                    className="check"
                    checked={selected.has(f)}
                    onChange={() => toggle(f)}
                  />
                  <span className="dupname">{f}</span>
                </label>
              ))}
            </div>
          ))}
          <div className="card note" style={{ marginTop: 14, marginBottom: 0 }}>
            Tick files anywhere on this page, review the list, and move them
            to the recycle bin — recoverable, never a silent delete.
          </div>
        </div>
      )}
      {!confirming && (
        <ReviewBar
          count={selPaths.length}
          bytes={selBytes}
          cta="Review & delete"
        onReview={() => {
          setReport(null);
          setConfirming(true);
        }}
        onClear={() => setSelected(new Set())}
        />
      )}
      {confirming && (
        <ConfirmModal
          paths={selPaths}
          bytes={selBytes}
          busy={busy === "trash"}
          report={report}
          action="trash"
          onConfirm={() => void doTrash()}
          onClose={closeModal}
        />
      )}
    </div>
  );
}

function useElevated() {
  const [elevated, setElevated] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<boolean>("is_elevated")
      .then(setElevated)
      .catch(() => setElevated(false));
  }, []);
  return elevated;
}

function AdminBanner(props: { onRestart: () => void; busy: boolean }) {
  return (
    <div className="card banner">
      <div>
        <b>Administrator rights needed.</b>
        <span className="muted">
          {" "}This module touches system folders. Scanning works now; moving
          files needs an elevated restart.
        </span>
      </div>
      <button className="btn primary" onClick={props.onRestart} disabled={props.busy}>
        {props.busy ? "Restarting…" : "Restart as administrator"}
      </button>
    </div>
  );
}

function OrphansScreen() {
  const elevated = useElevated();
  const [report, setReport] = useState<OrphanReport | null>(null);
  const [backup, setBackup] = useState<FileEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [selBackup, setSelBackup] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState<null | "backup" | "restore">(null);
  const [rep, setRep] = useState<TrashReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [unsupported, setUnsupported] = useState(false);

  async function refresh() {
    const r = await invoke<OrphanReport>("scan_installer_orphans");
    setReport(r);
    const b = await invoke<FileEntry[]>("list_orphan_backup").catch(
      () => [] as FileEntry[]
    );
    setBackup(b);
  }

  async function runScan() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await refresh();
    } catch (e) {
      const msg = errText(e);
      if (msg.includes("Windows only")) setUnsupported(true);
      else setError(msg);
    } finally {
      setBusy(false);
    }
  }

  async function goElevated() {
    setBusy(true);
    try {
      await invoke("restart_elevated");
      await getCurrentWindow().close();
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  }

  function toggle(set: Set<string>, apply: (s: Set<string>) => void, path: string) {
    const c = new Set(set);
    if (c.has(path)) c.delete(path);
    else c.add(path);
    apply(c);
  }

  const selPaths = [...selected];
  const selBytes = selPaths.reduce((s, p) => {
    const f = report?.orphans.find((o) => o.path === p);
    return s + (f?.size ?? 0);
  }, 0);
  const selBakPaths = [...selBackup];
  const selBakBytes = selBakPaths.reduce((s, p) => {
    const f = backup.find((o) => o.path === p);
    return s + (f?.size ?? 0);
  }, 0);

  async function doBackup() {
    if (selPaths.length === 0 || busy) return;
    setBusy(true);
    try {
      const r = await invoke<TrashReport>("backup_orphans", { paths: selPaths });
      setRep(r);
      await refresh();
    } catch (e) {
      setError(errText(e));
      setConfirming(null);
    } finally {
      setBusy(false);
    }
  }

  async function doRestore() {
    if (selBakPaths.length === 0 || busy) return;
    setBusy(true);
    try {
      const r = await invoke<TrashReport>("restore_orphans", { paths: selBakPaths });
      setRep(r);
      await refresh();
    } catch (e) {
      setError(errText(e));
      setConfirming(null);
    } finally {
      setBusy(false);
    }
  }

  function closeModal() {
    setConfirming(null);
    setRep(null);
    setSelected(new Set());
    setSelBackup(new Set());
  }

  return (
    <div>
      <div className="topbar">
        <div>
          <h1>Installer Orphans</h1>
          <p>Cached installers nothing references — move to backup first.</p>
        </div>
        <span className="vtag">v0.4</span>
      </div>
      {unsupported ? (
        <div className="card note">
          This module reads the Windows installer cache, so it only runs on
          Windows.
        </div>
      ) : (
        <>
          {elevated === false && (
            <AdminBanner onRestart={() => void goElevated()} busy={busy} />
          )}
          <div className="card hero">
            <div className="scanrow">
              <button className="btn primary" onClick={() => void runScan()} disabled={busy}>
                {busy ? "Scanning…" : "Scan installer cache"}
              </button>
            </div>
            {error !== "" && <div className="err">{error}</div>}
          </div>
          {report && (
            <>
              <div className="stats">
                <Stat label="Orphaned" value={fmtBytes(report.totalBytes)} />
                <Stat label="Orphan files" value={fmtInt(report.orphans.length)} />
                <Stat label="Cached files" value={fmtInt(report.scanned)} />
                <Stat label="Referenced" value={fmtInt(report.referenced)} />
              </div>
              <div className="card tablecard">
                <h2>Orphans — select to back up</h2>
                {report.orphans.length === 0 ? (
                  <p className="muted">No orphans. The cache is fully referenced.</p>
                ) : (
                  <table className="dtable">
                    <thead>
                      <tr>
                        <th className="checkcol" />
                        <th>Name</th>
                        <th className="num">Size</th>
                      </tr>
                    </thead>
                    <tbody>
                      {report.orphans.map((o) => (
                        <tr key={o.path} className={selected.has(o.path) ? "picked" : ""}>
                          <td>
                            <input
                              type="checkbox"
                              className="check"
                              checked={selected.has(o.path)}
                              onChange={() => toggle(selected, setSelected, o.path)}
                              aria-label={`Select ${o.name}`}
                            />
                          </td>
                          <td>
                            <span className="fname" title={o.path}>
                              {o.name}
                            </span>
                          </td>
                          <td className="num">{fmtBytes(o.size)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
              {selected.size > 0 && (
                <ReviewBar
                  count={selPaths.length}
                  bytes={selBytes}
                  cta="Review & back up"
                  onReview={() => {
                    setRep(null);
                    setConfirming("backup");
                  }}
                  onClear={() => setSelected(new Set())}
                />
              )}
              <div className="card">
                <h2>Backup folder</h2>
                {backup.length === 0 ? (
                  <p className="muted">Nothing backed up yet.</p>
                ) : (
                  <table className="dtable">
                    <thead>
                      <tr>
                        <th className="checkcol" />
                        <th>Name</th>
                        <th className="num">Size</th>
                      </tr>
                    </thead>
                    <tbody>
                      {backup.map((f) => (
                        <tr key={f.path} className={selBackup.has(f.path) ? "picked" : ""}>
                          <td>
                            <input
                              type="checkbox"
                              className="check"
                              checked={selBackup.has(f.path)}
                              onChange={() => toggle(selBackup, setSelBackup, f.path)}
                              aria-label={`Select ${f.name}`}
                            />
                          </td>
                          <td>
                            <span className="fname" title={f.path}>
                              {f.name}
                            </span>
                          </td>
                          <td className="num">{fmtBytes(f.size)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
                {selBackup.size > 0 && (
                  <div className="btnrow">
                    <button
                      className="btn primary"
                      onClick={() => {
                        setRep(null);
                        setConfirming("restore");
                      }}
                    >
                      Restore {selBackup.size} from backup
                    </button>
                  </div>
                )}
              </div>
            </>
          )}
          {confirming !== null && (
            <ConfirmModal
              action={confirming}
              paths={confirming === "backup" ? selPaths : selBakPaths}
              bytes={confirming === "backup" ? selBytes : selBakBytes}
              busy={busy}
              report={rep}
              onConfirm={() => void (confirming === "backup" ? doBackup() : doRestore())}
              onClose={closeModal}
            />
          )}
        </>
      )}
    </div>
  );
}

function SweepScreen() {
  const { roots, path, setPath } = useRoots((r) => r.temp || r.home || r.roots[0] || "");
  const [days, setDays] = useState("30");
  const [rep, setRep] = useState<RemnantReport | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [report, setReport] = useState<TrashReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function runScan() {
    if (!path || busy) return;
    setBusy(true);
    setError("");
    setRep(null);
    try {
      const n = Math.max(0, parseInt(days, 10) || 0);
      const r = await invoke<RemnantReport>("scan_remnants", {
        options: { root: path, minAgeDays: n },
      });
      setRep(r);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  }

  function toggle(path: string) {
    setSelected((s) => {
      const c = new Set(s);
      if (c.has(path)) c.delete(path);
      else c.add(path);
      return c;
    });
  }

  const selPaths = [...selected];
  const selBytes = selPaths.reduce((s, p) => {
    const a = rep?.emptyDirs.find((e) => e.path === p);
    if (a) return s + a.size;
    const b = rep?.staleFiles.find((e) => e.path === p);
    return s + (b?.size ?? 0);
  }, 0);

  async function doTrash() {
    if (selPaths.length === 0 || busy) return;
    setBusy(true);
    try {
      const r = await invoke<TrashReport>("trash_delete", { paths: selPaths });
      setReport(r);
      const gone = new Set(r.results.filter((x) => x.ok).map((x) => x.path));
      if (gone.size > 0) {
        setRep((d) =>
          d === null
            ? d
            : {
                ...d,
                emptyDirs: d.emptyDirs.filter((e) => !gone.has(e.path)),
                staleFiles: d.staleFiles.filter((e) => !gone.has(e.path)),
              }
        );
      }
    } catch (e) {
      setError(errText(e));
      setConfirming(false);
    } finally {
      setBusy(false);
    }
  }

  function closeModal() {
    setConfirming(false);
    setReport(null);
    setSelected(new Set());
  }

  function remnantTable(entries: RemnantEntry[], showSize: boolean) {
    if (entries.length === 0) return <p className="muted">None found.</p>;
    return (
      <table className="dtable">
        <thead>
          <tr>
            <th className="checkcol" />
            <th>Name</th>
            {showSize && <th className="num">Size</th>}
          </tr>
        </thead>
        <tbody>
          {entries.slice(0, 100).map((e) => (
            <tr key={e.path} className={selected.has(e.path) ? "picked" : ""}>
              <td>
                <input
                  type="checkbox"
                  className="check"
                  checked={selected.has(e.path)}
                  onChange={() => toggle(e.path)}
                  aria-label={`Select ${e.name}`}
                />
              </td>
              <td>
                <span className="fname" title={e.path}>
                  {e.name}
                </span>
              </td>
              {showSize && <td className="num">{fmtBytes(e.size)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    );
  }

  return (
    <div>
      <div className="topbar">
        <div>
          <h1>Remnant Sweep</h1>
          <p>Empty folders and stale temp files — preview, then trash.</p>
        </div>
        <span className="vtag">v0.4</span>
      </div>
      <div className="card hero">
        <div className="scanrow">
          <input
            className="pathinput"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Folder to sweep…"
            spellCheck={false}
          />
          <button className="btn primary" onClick={() => void runScan()} disabled={busy || !path}>
            {busy ? "Sweeping…" : "Sweep"}
          </button>
        </div>
        <div className="agerow">
          Temp files older than
          <input
            className="ageinput"
            value={days}
            onChange={(e) => setDays(e.target.value)}
            inputMode="numeric"
            aria-label="Minimum age in days"
          />
          days (.tmp, .log, .bak)
        </div>
        <RootPills roots={roots} path={path} setPath={setPath} />
        {busy && (
          <div className="loading">
            <span className="spin" />
            Sweeping{path ? ` ${path}` : ""}…
          </div>
        )}
        {error !== "" && <div className="err">{error}</div>}
      </div>
      {rep && (
        <div className="cols">
          <div className="card">
            <h2>Empty folders ({rep.emptyDirs.length})</h2>
            {remnantTable(rep.emptyDirs, false)}
          </div>
          <div className="card">
            <h2>Stale temp files ({rep.staleFiles.length})</h2>
            {remnantTable(rep.staleFiles, true)}
          </div>
        </div>
      )}
      {!confirming && (
        <ReviewBar
          count={selPaths.length}
          bytes={selBytes}
          cta="Review & delete"
          onReview={() => {
            setReport(null);
            setConfirming(true);
          }}
          onClear={() => setSelected(new Set())}
        />
      )}
      {confirming && (
        <ConfirmModal
          paths={selPaths}
          bytes={selBytes}
          busy={busy}
          report={report}
          action="trash"
          onConfirm={() => void doTrash()}
          onClose={closeModal}
        />
      )}
    </div>
  );
}

function About() {
  return (
    <div>
      <div className="topbar">
        <div>
          <h1>About</h1>
          <p>What this app is.</p>
        </div>
      </div>
      <div className="card about">
        <img src="/icon.png" alt="OS Cleaner" className="aboutlogo" />
        <h2>OS Cleaner</h2>
        <p className="muted">by Don Pely · v0.4.0 · MIT License</p>
        <p>
          A cross-platform system cleaner and disk analyzer. v0.4 hunts
          installer orphans and remnant files with backup-first safety —
          plus dark mode.
        </p>
        <button
          className="btn"
          onClick={() => openUrl(REPO).catch(() => {})}
        >
          View on GitHub
        </button>
      </div>
    </div>
  );
}
