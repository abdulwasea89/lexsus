import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import { Toaster } from "./components/ui/toast";
import { TooltipProvider } from "./components/ui/tooltip";
import { applyTheme, loadTheme } from "./hooks/useTheme";
import "./index.css";
import "./App.css";

applyTheme(loadTheme());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary label="Lexsus">
      <TooltipProvider>
        <Toaster>
          <App />
        </Toaster>
      </TooltipProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
