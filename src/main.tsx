import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import VideoApp from "./VideoApp";
import { JobsTray } from "./Jobs";
import "./styles.css";
import "./video.css";

type Mode = "photo" | "video";

function Root() {
  const [mode, setMode] = useState<Mode>("photo");
  const sw = (
    <div className="mode-switch seg" role="tablist" aria-label="Workspace">
      <button role="tab" aria-selected={mode === "photo"} className={mode === "photo" ? "on" : ""} onClick={() => setMode("photo")}>Photos</button>
      <button role="tab" aria-selected={mode === "video"} className={mode === "video" ? "on" : ""} onClick={() => setMode("video")}>Video</button>
    </div>
  );
  // Both stay mounted so switching never loses your work.
  return (
    <>
      <App modeSwitch={sw} active={mode === "photo"} />
      <VideoApp modeSwitch={sw} active={mode === "video"} />
      <JobsTray />
    </>
  );
}

// Windows / Linux draw their own title bar, so there are no Mac window buttons to leave room for
if (!/Mac/.test(navigator.platform || navigator.userAgent)) document.documentElement.classList.add("not-mac");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
