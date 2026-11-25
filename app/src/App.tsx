import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

interface ScanNode {
  name: string;
  path: string;
  size: number;
  files: number;
  dirs: number;
  errors: number;
  children: ScanNode[];
}

interface Roots {
  roots: string[];
  home: string;
}

type View = "care" | "disk" | "about";

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

function errText(e: unknown): string {
  return typeof e === "string" ? e : String(e);
}

const REPO = "https://github.com/yourbossdonpely/oscleaner";

export default function App() {
  const [view, setView] = useState<View>("care");
  return (
    <div className="app">
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
          <button className="navbtn" disabled title="Coming in v0.2">
            <span className="ni">⧉</span>My Clutter<span className="soon">soon</span>
          </button>
          <button className="navbtn" disabled title="Coming in v0.2">
            <span className="ni">◧</span>App Leftovers<span className="soon">soon</span>
          </button>
          <div className="cap">APP</div>
          <NavBtn id="about" view={view} setView={setView} label="About" icon="ⓘ" />
        </nav>
        <div className="sidefoot">v0.1.0 · scan + insight</div>
      </aside>
      <main className="main">
        {view === "care" && <SmartCare />}
        {view === "disk" && <DiskInsight />}
        {view === "about" && <About />}
      </main>
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
      <div className="topbar">
        <div>
          <h1>Smart Care</h1>
          <p>Scan everything, see what&apos;s eating your disk.</p>
        </div>
        <span className="vtag">v0.1</span>
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
            v0.1 scans and shows. Automatic cleanup with backup-first safety
            lands in v0.2 — <b>nothing here deletes anything.</b>
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
          <p>Browse folders by real size. Click ▸ to expand.</p>
        </div>
        <span className="vtag">v0.1</span>
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
        <p className="muted">by Don Pely · v0.1.0 · MIT License</p>
        <p>
          A cross-platform system cleaner and disk analyzer. v0.1 scans
          folders and shows what&apos;s eating your disk — hardlink-aware,
          trash-first, nothing deleted without you.
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
