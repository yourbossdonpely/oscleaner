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

interface Roots {
  roots: string[];
  home: string;
}

type View = "care" | "disk" | "clutter" | "about";

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
  return (
    <div className={`app ${THEMES[view]}`}>
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
          <button className="navbtn" disabled title="Coming in v0.3">
            <span className="ni">◧</span>App Leftovers<span className="soon">soon</span>
          </button>
          <div className="cap">APP</div>
          <NavBtn id="about" view={view} setView={setView} label="About" icon="ⓘ" />
        </nav>
        <div className="sidefoot">v0.2.0 · scan + insight</div>
      </aside>
      <main className="main">
        {view === "care" && <SmartCare />}
        {view === "disk" && <DiskInsight />}
        {view === "clutter" && <MyClutter />}
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
  const [roots, setRoots] = useState<Roots>({ roots: [], home: "" });
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
        <span className="vtag">v0.2</span>
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
        <span className="vtag">v0.2</span>
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

function FileTable({ files, showDate }: { files: FileEntry[]; showDate: boolean }) {
  const shown = files.slice(0, 25);
  return (
    <div>
      <table className="dtable">
        <thead>
          <tr>
            <th>Name</th>
            <th className="num">Size</th>
            {showDate && <th className="num">Modified</th>}
          </tr>
        </thead>
        <tbody>
          {shown.map((f) => (
            <tr key={f.path}>
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

function MyClutter() {
  const { roots, path, setPath } = useRoots((r) => r.home || r.roots[0] || "");
  const [scan, setScan] = useState<ScanNode | null>(null);
  const [dups, setDups] = useState<DupReport | null>(null);
  const [busy, setBusy] = useState<"scan" | "dups" | null>(null);
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

  const waste =
    dups?.groups.reduce((s, g) => s + g.size * (g.files.length - 1), 0) ?? 0;

  return (
    <div>
      <div className="topbar">
        <div>
          <h1>My Clutter</h1>
          <p>Large files, old files, and true duplicates by content hash.</p>
        </div>
        <span className="vtag">v0.2</span>
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
            <FileTable files={scan.largest} showDate={false} />
          </div>
          <div className="card">
            <h2>Oldest files</h2>
            <FileTable files={scan.oldest} showDate={true} />
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
                <div key={f} className="dupfile" title={f}>
                  {f}
                </div>
              ))}
            </div>
          ))}
          <div className="card note" style={{ marginTop: 14, marginBottom: 0 }}>
            Read-only in v0.2 — select-and-delete with trash-first safety
            lands in v0.3.
          </div>
        </div>
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
        <p className="muted">by Don Pely · v0.2.0 · MIT License</p>
        <p>
          A cross-platform system cleaner and disk analyzer. v0.2 scans
          folders, maps them as a treemap, and finds large, old, and
          duplicate files — hardlink-aware, trash-first, nothing deleted
          without you.
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
