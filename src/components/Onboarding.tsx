"use client";

import { useEffect, useState } from "react";
import {
  ActivityIcon,
  ArrowLeftIcon,
  ArrowRightIcon,
  BrainIcon,
  FolderOpenIcon,
  GitBranchIcon,
  GlobeIcon,
  MessageCircleIcon,
  ShieldCheckIcon,
} from "lucide-react";
import { Button } from "./ui/button";
import { cn } from "../lib/utils";
import { markOnboarded } from "../lib/onboarding";

const TOUR: {
  icon: typeof ActivityIcon;
  eyebrow: string;
  title: string;
  desc: string;
}[] = [
  {
    icon: FolderOpenIcon,
    eyebrow: "Workspace",
    title: "Bind a project",
    desc: "Pick the folder your web AI will work on. Every file it reads or edits is resolved against it — nothing happens outside.",
  },
  {
    icon: ActivityIcon,
    eyebrow: "Live activity trace",
    title: "Watch every move",
    desc: "Reads, edits and commands stream in as they happen, so you always see exactly what the AI is doing and why.",
  },
  {
    icon: ShieldCheckIcon,
    eyebrow: "Approvals",
    title: "Approve before it acts",
    desc: "Writes and shell commands pause for your call. Allow, deny, or grant for the session — you stay in control.",
  },
  {
    icon: GitBranchIcon,
    eyebrow: "Git",
    title: "Review and commit",
    desc: "Inspect the diff, switch branches and commit the AI's work without ever leaving the workbench.",
  },
  {
    icon: MessageCircleIcon,
    eyebrow: "Handoff",
    title: "Continue anywhere",
    desc: "Package your progress into a clean handoff so another AI can pick up the thread without losing context.",
  },
  {
    icon: BrainIcon,
    eyebrow: "Project memory",
    title: "Remember decisions",
    desc: "Facts, decisions and dead ends are saved across sessions, so nobody re-litigates the same problem twice.",
  },
  {
    icon: GlobeIcon,
    eyebrow: "Web-AI connector",
    title: "Connect your AI",
    desc: "Point your web AI at the local endpoint. It binds loopback only — read-only until you enable writes.",
  },
];

/**
 * First-run experience: a black stage that spells out "hello" letter by
 * letter, then a skippable tour of the workbench features. Completion is
 * persisted under the shared onboarding key, which also retires the
 * in-app GettingStarted panel.
 */
const HELLO = "hello";

export default function Onboarding({ onDone }: { onDone: () => void }) {
  const [phase, setPhase] = useState<"hello" | "tour">("hello");
  const [index, setIndex] = useState(0);
  const [typed, setTyped] = useState(0);
  const helloDone = typed >= HELLO.length;

  // Typewriter: reveal one character at a time, then stop.
  useEffect(() => {
    if (phase !== "hello" || helloDone) return;
    const delay = typed === 0 ? 350 : 170;
    const t = setTimeout(() => setTyped((n) => n + 1), delay);
    return () => clearTimeout(t);
  }, [phase, typed, helloDone]);

  function finish() {
    markOnboarded();
    onDone();
  }

  return (
    <div className="onboard-screen fixed inset-0 z-[60] flex flex-col overflow-hidden bg-black text-white">
      {phase === "hello" ? (
        <div className="relative flex flex-1 flex-col items-center justify-center px-6">
          <div aria-hidden className="onboard-glow" />
          <h1 className="onboard-hello" aria-label="hello">
            {/* Invisible full word reserves the width so the word types
                left-to-right without the centred heading shifting. */}
            <span aria-hidden className="invisible">
              {HELLO}
            </span>
            <span aria-hidden className="absolute inset-0">
              {HELLO.slice(0, typed)}
            </span>
          </h1>
          <p
            className="anim-fade-up mt-6 text-center text-sm text-white/50"
            style={{ animationPlayState: helloDone ? "running" : "paused" }}
          >
            Your local bridge between a web AI and your codebase.
          </p>
          <Button
            size="lg"
            onClick={() => setPhase("tour")}
            className="anim-fade-up mt-12 h-11 px-6 text-sm"
            style={{
              animationDelay: "90ms",
              animationPlayState: helloDone ? "running" : "paused",
            }}
          >
            Get Started
            <ArrowRightIcon className="ml-1.5 size-4" />
          </Button>
        </div>
      ) : (
        <div className="flex flex-1 flex-col items-center justify-center px-6">
          <div className="w-full max-w-lg">
            <div className="flex items-center justify-between">
              <span className="app-eyebrow text-white/35">
                {index + 1} / {TOUR.length}
              </span>
              <Button
                variant="ghost"
                size="sm"
                onClick={finish}
                className="text-white/50 hover:bg-white/10 hover:text-white"
              >
                Skip tour
              </Button>
            </div>

            <div
              key={index}
              className="anim-fade-up mt-10 flex flex-col items-start gap-4"
            >
              <span className="flex size-14 items-center justify-center rounded-2xl bg-white/5 text-primary ring-1 ring-white/10">
                {(() => {
                  const Icon = TOUR[index].icon;
                  return <Icon className="size-6" />;
                })()}
              </span>
              <span className="app-eyebrow text-primary/80">
                {TOUR[index].eyebrow}
              </span>
              <h2 className="text-3xl font-semibold tracking-tight text-white">
                {TOUR[index].title}
              </h2>
              <p className="max-w-md text-sm leading-relaxed text-white/55">
                {TOUR[index].desc}
              </p>
            </div>

            <div className="mt-12 flex items-center justify-between">
              <div className="flex items-center gap-1.5">
                {TOUR.map((slide, i) => (
                  <button
                    key={slide.title}
                    type="button"
                    aria-label={`Go to ${slide.title}`}
                    aria-current={i === index}
                    onClick={() => setIndex(i)}
                    className={cn(
                      "h-1.5 rounded-full transition-all duration-300 ease-out",
                      i === index
                        ? "w-6 bg-primary"
                        : "w-1.5 bg-white/20 hover:bg-white/40",
                    )}
                  />
                ))}
              </div>

              <div className="flex items-center gap-2">
                {index > 0 && (
                  <Button
                    variant="ghost"
                    onClick={() => setIndex((i) => i - 1)}
                    className="text-white/60 hover:bg-white/10 hover:text-white"
                  >
                    <ArrowLeftIcon className="mr-1 size-4" />
                    Back
                  </Button>
                )}
                {index < TOUR.length - 1 ? (
                  <Button onClick={() => setIndex((i) => i + 1)}>
                    Next
                    <ArrowRightIcon className="ml-1 size-4" />
                  </Button>
                ) : (
                  <Button onClick={finish}>Get Started</Button>
                )}
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
