import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  CopyMinusIcon,
  CopyPlusIcon,
  MoonIcon,
  SunIcon,
  XIcon,
} from "lucide-react";
import { toggleTheme, useTheme } from "../hooks/useTheme";
import { Button } from "./ui/button";

const appWindow = getCurrentWindow();

/**
 * Custom titlebar replacing the system (GTK) headerbar: title on the
 * left, window controls on the right, the whole strip draggable via
 * `data-tauri-drag-region` (works on X11 and Wayland).
 */
export default function Titlebar() {
  const theme = useTheme();

  return (
    <header
      data-tauri-drag-region
      className="glass-sidebar flex h-10 shrink-0 select-none items-center border-b pl-4"
    >
      <p
        data-tauri-drag-region
        className="text-sm font-semibold tracking-tight text-foreground"
      >
        Lexsus
      </p>

      <div className="ml-auto flex h-full items-stretch">
        <Button
          variant="ghost"
          size="sm"
          aria-label={theme === "dark" ? "Use light theme" : "Use dark theme"}
          title={theme === "dark" ? "Use light theme" : "Use dark theme"}
          className="h-full rounded-none px-3.5 hover:bg-muted"
          onClick={toggleTheme}
        >
          {theme === "dark" ? (
            <SunIcon className="size-3.5" />
          ) : (
            <MoonIcon className="size-3.5" />
          )}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          aria-label="Minimize"
          className="h-full rounded-none px-3.5 hover:bg-muted"
          onClick={() => void appWindow.minimize()}
        >
          <CopyMinusIcon className="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="sm"
          aria-label="Maximize"
          className="h-full rounded-none px-3.5 hover:bg-muted"
          onClick={() => void appWindow.toggleMaximize()}
        >
          <CopyPlusIcon className="size-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="sm"
          aria-label="Close"
          className="h-full rounded-none px-3.5 hover:bg-danger hover:text-danger-foreground"
          onClick={() => void appWindow.close()}
        >
          <XIcon className="size-4" />
        </Button>
      </div>
    </header>
  );
}
