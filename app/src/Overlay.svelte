<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import type { Card, Status, VerdictLabel } from "./lib/types";

  let card = $state<Card | null>(null);
  let leaving = $state(false);
  let status = $state<Status | null>(null);
  let hideTimer: ReturnType<typeof setTimeout> | null = null;
  let statusTimer: ReturnType<typeof setTimeout> | null = null;

  const colors: Record<VerdictLabel, string> = {
    UPGRADE: "var(--color-le-upgrade)",
    SIDEGRADE: "var(--color-le-side)",
    WORSE: "var(--color-le-worse)",
    REVIEW: "var(--color-le-review)",
    UNKNOWN: "#9ca3af",
  };

  function dismiss() {
    if (!card || leaving) return;
    leaving = true;
    setTimeout(() => {
      card = null;
      leaving = false;
    }, 220);
  }

  function show(next: Card) {
    if (hideTimer) clearTimeout(hideTimer);
    leaving = false;
    card = next;
    // seconds = 0: no timer, the backend dismisses the card when the mouse moves
    if (next.seconds > 0) hideTimer = setTimeout(dismiss, next.seconds * 1000);
  }

  function showStatus(next: Status) {
    if (statusTimer) clearTimeout(statusTimer);
    status = next;
    statusTimer = setTimeout(() => (status = null), next.kind === "error" ? 4500 : 1800);
  }

  function fmt(n: number, digits = 2) {
    return (n >= 0 ? "+" : "") + n.toFixed(digits);
  }

  onMount(() => {
    const unlisten = [
      listen<Card>("verdict", (e) => show(e.payload)),
      listen<void>("dismiss", () => dismiss()),
      listen<Status>("status", (e) => showStatus(e.payload)),
    ];
    return () => {
      unlisten.forEach((p) => p.then((f) => f()));
    };
  });
</script>

{#if card}
  {@const v = card.verdict}
  {@const color = colors[v.label]}
  <div
    class="glass absolute left-3 right-3 top-3 rounded-xl p-3.5 text-[13px] text-zinc-100 {leaving ? 'card-leave' : 'card-enter'}"
    style="border-color: color-mix(in srgb, {color} 55%, transparent);"
  >
    <div class="flex items-baseline justify-between gap-3">
      <div class="min-w-0 truncate text-[17px] font-bold tracking-wide" style="color: {color}">{v.label}</div>
      {#if v.label !== "UNKNOWN"}
        <div class="shrink-0 whitespace-nowrap font-mono text-[13px] text-zinc-300">
          <span style="color: {color}">{fmt(v.delta)}</span>
          <span class="text-zinc-500"> · {v.candidate_score.toFixed(2)} vs {v.equipped_score.toFixed(2)}</span>
        </div>
      {/if}
    </div>
    {#if !card.ai && card.ai_pending}
      <div class="mt-1 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-le-purple">
        <span class="inline-flex items-center gap-1.5 rounded-full border border-le-purple/40 bg-le-purple/10 px-1.5 py-px"><span class="spin"></span> asking {card.ai_pending}…</span>
      </div>
    {/if}
    {#if card.ai}
      <div class="mt-1 flex flex-wrap items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-le-purple">
        <span class="rounded-full border border-le-purple/60 bg-le-purple/15 px-1.5 py-px">AI · {card.ai.model}</span>
        {#if card.ai.confidence}<span class="rounded-full border border-le-purple/40 px-1.5 py-px">{card.ai.confidence} confidence</span>{/if}
        {#if card.ai.cached}<span class="rounded-full border border-zinc-600 px-1.5 py-px text-zinc-400">cached</span>{/if}
      </div>
    {/if}

    <div class="mt-0.5 truncate text-[13px] text-le-gold">{v.candidate_name}</div>
    <div class="truncate text-[11px] text-zinc-400">
      {card.subtitle}{#if v.equipped_name} · vs {v.equipped_name}{/if}{#if card.equipped_from_tooltip} <span class="text-zinc-500">(read from compare tooltip)</span>{/if}
    </div>

    {#if card.ai?.summary}
      <div class="mt-2 rounded-md border border-le-purple/25 bg-le-purple/10 px-2 py-1.5 text-[12px] leading-snug text-zinc-100">
        {card.ai.summary}
      </div>
    {/if}

    {#if v.reasons.length}
      <div class="mt-2 h-px bg-le-gold/25"></div>
      <ul class="mt-2 space-y-1 text-[12px]">
        {#each v.reasons.slice(0, card.ai ? 4 : 3) as reason}
          <li class="flex gap-2">
            <span class="mt-[7px] h-1 w-1 shrink-0 rounded-full" style="background: {reason.trim().startsWith('-') ? 'var(--color-le-worse)' : 'var(--color-le-upgrade)'}"></span>
            <span class="leading-snug text-zinc-200">{reason}</span>
          </li>
        {/each}
      </ul>
    {/if}

    {#if v.warnings.length}
      <div class="mt-2 space-y-0.5 text-[11px] text-le-side">
        {#each v.warnings.slice(0, 3) as warning}
          <div class="leading-snug">⚠ {warning}</div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

{#if status}
  <div
    class="glass card-enter absolute bottom-2 right-2 max-w-[376px] rounded-lg px-3 py-1.5 text-[12px] {status.kind === 'error' ? 'text-le-worse' : 'text-zinc-300'}"
  >
    {status.text}
  </div>
{/if}
