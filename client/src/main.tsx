/**
 * main.tsx — Application Entry Point
 *
 * Mounts the root React component (<App />) into the DOM element with id "root".
 * React.StrictMode is enabled to highlight potential problems during development.
 */

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
