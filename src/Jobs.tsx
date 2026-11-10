import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export type Job = {
  id: number; kind: string; title: string; progress: number;
  state: "running" | "done" | "failed" | "cancelled"; message?: string | null; outputs: string[]; started: number;
};

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/** Background jobs: a button in the corner that opens the list of exports running and done. */
export function JobsTray() {
  const [jobs, setJobs] = useState<Record<number, Job>>({});
  const [open, setOpen] = useState(false);
  const [flash, setFlash] = useState(false);

  useEffect(() => {
    invoke<Job[]>("jobs_list").then((l) => setJobs(Object.fromEntries((l ?? []).map((j) => [j.id, j])))).catch(() => {});
    const un = listen<Job>("job", (e) => {
      setJobs((m) => {
        const prev = m[e.payload.id];
        if (!prev) setOpen(true); // show the tray when a new job starts
        if (prev?.state === "running" && e.payload.state !== "running") {
          setFlash(true);
          setTimeout(() => setFlash(false), 1600);
        }
        return { ...m, [e.payload.id]: e.payload };
      });
    });
    return () => void un.then((f) => f());
  }, []);

  const list = Object.values(jobs).sort((a, b) => b.id - a.id);
  if (!list.length) return null;
  const running = list.filter((j) => j.state === "running");
  const clear = async () => {
    await invoke("jobs_clear");
    setJobs((m) => Object.fromEntries(Object.entries(m).filter(([, j]) => j.state === "running")));
  };

  return (
    <div className={`jobs ${open ? "open" : ""}`}>
      {open && (
        <div className="jobs-panel" role="dialog" aria-label="Background jobs">
          <div className="jobs-head">
            <strong>Jobs</strong>
            <span className="muted">{running.length ? `${running.length} running` : "All done"}</span>
            {list.some((j) => j.state !== "running") && <button className="link" onClick={clear}>Clear finished</button>}
            <button className="link" onClick={() => setOpen(false)} aria-label="Hide jobs">Hide</button>
          </div>
          <ul>
            {list.map((j) => (
              <li key={j.id} className={`job ${j.state}`}>
                <div className="job-top">
                  <span className="job-title" title={j.title}>{j.title}</span>
                  {j.state === "running" ? (
                    <button className="link" onClick={() => invoke("job_cancel", { id: j.id })}>Stop</button>
                  ) : j.state === "done" && j.outputs[0] ? (
                    <button className="link" onClick={() => revealItemInDir(j.outputs[0])}>Show in Finder</button>
                  ) : null}
                </div>
                {j.state === "running" && (
                  <div className={`job-bar ${j.progress < 0 ? "indeterminate" : ""}`}>
                    <span style={{ width: j.progress < 0 ? undefined : `${Math.round(j.progress * 100)}%` }} />
                  </div>
                )}
                <div className="job-msg">
                  {j.state === "running" && (j.message ? j.message : j.progress >= 0 ? `${Math.round(j.progress * 100)}%` : "Working…")}
                  {j.state === "done" && (j.outputs.length > 1 ? `Saved ${j.outputs.length} files` : j.outputs[0] ? `Saved ${baseName(j.outputs[0])}` : "Done")}
                  {j.state === "failed" && <span className="err">{j.message}</span>}
                  {j.state === "cancelled" && "Stopped"}
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
      <button className={`jobs-btn ${flash ? "flash" : ""}`} onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        {running.length ? <span className="jobs-spin" aria-hidden /> : <span className="jobs-dot" aria-hidden />}
        {running.length ? `${running.length} job${running.length > 1 ? "s" : ""} running` : "Jobs"}
      </button>
    </div>
  );
}
