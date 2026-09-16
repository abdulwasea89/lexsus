import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { bridgeAnswerQuestion } from "../lib/bridge";

/** A card `ask_user` or `propose_plan` put on the desktop. */
export interface QuestionCard {
  id: number;
  kind: "question" | "plan";
  title: string;
  body: string;
  options: string[];
  source: string;
}

/**
 * Owns the agent-loop question queue. The Rust side emits
 * `bridge://question-requested` and blocks the MCP call until
 * `bridge_answer_question` resolves it, exactly like an approval — so a tool
 * cannot answer for the human by timing out silently.
 */
export function useQuestions() {
  const [questions, setQuestions] = useState<QuestionCard[]>([]);
  const mounted = useRef(false);

  useEffect(() => {
    mounted.current = true;
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      unlisten = await listen<QuestionCard>(
        "bridge://question-requested",
        (e) => {
          if (!mounted.current) return;
          setQuestions((prev) => [
            ...prev.filter((q) => q.id !== e.payload.id),
            e.payload,
          ]);
        },
      );
    })();
    return () => {
      mounted.current = false;
      unlisten?.();
    };
  }, []);

  async function answer(id: number, option: string | null, text: string) {
    setQuestions((prev) => prev.filter((q) => q.id !== id));
    await bridgeAnswerQuestion(id, option, text).catch(() => {});
  }

  return { questions, answer };
}
