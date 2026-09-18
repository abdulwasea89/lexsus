"use client";

import { FolderOpenIcon, GlobeIcon, ShieldCheckIcon } from "lucide-react";
import { Button } from "./ui/button";
import { isOnboarded, markOnboarded } from "../lib/onboarding";

const STEPS = [
  {
    icon: FolderOpenIcon,
    title: "Pick a project",
    desc: "Bind the folder the web AI will work on. Every file it touches is resolved against it.",
  },
  {
    icon: GlobeIcon,
    title: "Connect your AI",
    desc: "Point your web AI at the local MCP endpoint. It binds loopback only — read-only until you enable writes.",
  },
  {
    icon: ShieldCheckIcon,
    title: "Approve & watch",
    desc: "When a write or command is requested, the approval banner lets you Allow, Deny, or grant it for the session.",
  },
];

export default function GettingStarted({
  onOpenProject,
}: {
  onOpenProject: () => void;
}) {
  if (isOnboarded()) return null;

  function dismiss() {
    markOnboarded();
  }

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center p-6">
      <div className="flex w-full max-w-2xl flex-col gap-6 rounded-xl border border-border bg-surface p-6 shadow-sm">
        <div className="flex items-center justify-between gap-4">
          <div>
            <h2 className="app-display text-xl">Welcome to Lexsus</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Get started in three steps — then let the bridge do the rest.
            </p>
          </div>
          <Button variant="ghost" size="sm" onClick={dismiss}>
            Dismiss
          </Button>
        </div>

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          {STEPS.map((step, i) => (
            <div
              key={step.title}
              className="flex flex-col gap-2.5 rounded-lg border border-border/60 bg-surface-2/50 p-4 anim-fade-up"
              style={{ animationDelay: `${i * 80}ms` }}
            >
              <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                <step.icon className="size-4" />
              </span>
              <p className="text-sm font-semibold">{step.title}</p>
              <p className="text-xs leading-relaxed text-muted-foreground">
                {step.desc}
              </p>
            </div>
          ))}
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border pt-4">
          <Button onClick={() => void onOpenProject()}>
            <FolderOpenIcon className="mr-1.5 size-4" />
            Open a project
          </Button>
          <Button
            variant="outline"
            onClick={() => void dismiss()}
          >
            Skip for now
          </Button>
        </div>
      </div>
    </div>
  );
}
