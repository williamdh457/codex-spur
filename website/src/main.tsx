import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Site } from "./Site";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode><Site locale={window.location.pathname.startsWith("/en") ? "en" : "zh-CN"} /></StrictMode>,
);
