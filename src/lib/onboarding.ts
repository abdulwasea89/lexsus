export const ONBOARD_KEY = "lexsus.onboarded";

/** True once the intro + tour have been completed (or explicitly skipped). */
export function isOnboarded(): boolean {
  try {
    return localStorage.getItem(ONBOARD_KEY) === "true";
  } catch {
    return true;
  }
}

export function markOnboarded() {
  try {
    localStorage.setItem(ONBOARD_KEY, "true");
  } catch {
    // storage unavailable
  }
}
