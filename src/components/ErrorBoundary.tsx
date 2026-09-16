import { Component, type ErrorInfo, type ReactNode } from "react";
import { CircleAlertIcon } from "lucide-react";
import { Button } from "./ui/button";

interface ErrorBoundaryProps {
  /** Shown in the fallback, e.g. "Live activity trace". */
  label?: string;
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

/**
 * Contains a render error to the view that threw instead of unmounting the
 * whole app to a blank screen. "Reload view" clears the error so the child
 * mounts fresh; keying the boundary by active view makes switching tabs
 * recover automatically.
 */
export default class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("View crashed:", error, info);
  }

  reset = () => this.setState({ error: null });

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="flex h-full min-h-0 flex-col items-center justify-center gap-2 rounded-lg border border-border bg-surface p-8 text-center">
        <CircleAlertIcon className="size-8 text-danger" />
        <p className="text-sm font-medium">
          {this.props.label ?? "This view"} crashed
        </p>
        <p className="max-w-80 font-mono text-[11px] leading-relaxed text-muted-foreground">
          {error.message}
        </p>
        <Button size="sm" variant="outline" onClick={this.reset}>
          Reload view
        </Button>
      </div>
    );
  }
}
