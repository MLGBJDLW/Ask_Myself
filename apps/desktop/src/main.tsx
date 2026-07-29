import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "./lib/ThemeProvider";
import { runLocalStorageMigrations } from "./lib/localStorageMigrations";
import "@fontsource-variable/inter";
import "./index.css";

runLocalStorageMigrations();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <App />
    </ThemeProvider>
  </React.StrictMode>,
);
