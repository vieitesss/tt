import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { tauriClient } from "../api";
import { Menubar } from "./Menubar";
import { tauriMenubarHost } from "./host";
import "../motion.css";
import "./menubar.css";

const container = document.getElementById("root");
if (!container) throw new Error("missing #root element");

createRoot(container).render(
  <StrictMode>
    <Menubar client={tauriClient} host={tauriMenubarHost(tauriClient)} />
  </StrictMode>,
);
