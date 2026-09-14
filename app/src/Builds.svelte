<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import type { AiSettings, BuildSummary, CharacterState, GuideOutcome, ModelConfig, ModelInfo, ModelRef, Settings } from "./lib/types";

  const PROVIDERS: { id: string; name: string; env: string; url: string; models: string[] }[] = [
    { id: "anthropic", name: "Anthropic", env: "ANTHROPIC_API_KEY", url: "https://api.anthropic.com", models: ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5-20251001"] },
    { id: "openai", name: "OpenAI", env: "OPENAI_API_KEY", url: "https://api.openai.com/v1", models: ["gpt-5", "gpt-5-mini", "gpt-5-nano", "gpt-4.1"] },
    { id: "kimi", name: "Kimi (Moonshot)", env: "MOONSHOT_API_KEY", url: "https://api.moonshot.ai/v1", models: ["kimi-k2.5", "kimi-k2-thinking", "kimi-k2-0905-preview"] },
    { id: "deepseek", name: "DeepSeek", env: "DEEPSEEK_API_KEY", url: "https://api.deepseek.com/v1", models: ["deepseek-chat", "deepseek-reasoner"] },
    { id: "openai-compatible", name: "Custom server", env: "LLM_API_KEY", url: "", models: [] },
  ];
  const CUSTOM = "openai-compatible";

  let builds = $state<BuildSummary[]>([]);
  let settings = $state<Settings | null>(null);
  let ai = $state<AiSettings | null>(null);
  let envKeys = $state<Record<string, boolean>>({});
  let character = $state<CharacterState | null>(null);
  let profileDir = $state("");
  let toast = $state<{ text: string; kind: "info" | "error" } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  // guide form
  let characterName = $state("");
  let buildName = $state("");
  let source = $state("");
  let guideText = $state("");
  let force = $state(false);
  let analyzing = $state(false);
  let analysisResult = $state<{ text: string; kind: "info" | "error" } | null>(null);

  // AI settings
  let saveState = $state<"saved" | "saving" | "unsaved">("saved");
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let connecting = $state<Record<string, boolean>>({});
  let connectError = $state<Record<string, string>>({});
  let showCustom = $state(false);
  let showAdvanced = $state(false);
  /** which picker is in "other model id" mode */
  let customMode = $state<{ item: boolean; guide: boolean }>({ item: false, guide: false });
  let addToCycle = $state("");
  let charSaveTimer: ReturnType<typeof setTimeout> | null = null;

  const active = $derived(builds.find((b) => b.active) ?? null);
  const knownCharacters = $derived([...new Set(builds.filter((b) => !b.builtin).map((b) => b.character_name))]);
  const matchingBuild = $derived(
    builds.find((b) => !b.builtin && slug(b.character_name) === slug(characterName) && slug(b.build_name) === slug(buildName)) ?? null,
  );

  function slug(s: string) {
    return s.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  }

  function say(text: string, kind: "info" | "error" = "info") {
    if (toastTimer) clearTimeout(toastTimer);
    toast = { text, kind };
    toastTimer = setTimeout(() => (toast = null), kind === "error" ? 6000 : 2500);
  }

  function when(unix: number) {
    if (!unix) return "built-in";
    return new Date(unix * 1000).toLocaleString();
  }

  function providerName(id: string) {
    return PROVIDERS.find((p) => p.id === id)?.name ?? id;
  }

  function refLabel(r: ModelRef | null | undefined) {
    if (!r || !r.model) return "none";
    return `${providerName(r.provider)} · ${r.model}`;
  }

  function same(a: ModelRef, b: ModelRef) {
    return a.provider === b.provider && a.model === b.model;
  }

  function money(v: number | null | undefined) {
    if (v == null) return "?";
    return "$" + (v < 0.01 ? v.toFixed(4) : v.toFixed(2));
  }

  /** true when the provider has a typed key or one in the environment */
  function hasKey(provider: string) {
    return !!ai?.providers[provider]?.api_key.trim() || !!envKeys[provider];
  }

  function keyStatus(provider: string) {
    if (ai?.providers[provider]?.api_key.trim()) return "key set";
    if (envKeys[provider]) return "key from environment";
    return "no key";
  }

  /** models offered for a provider: fetched catalogue, else the suggestions */
  function modelsOf(provider: string): { id: string; info: ModelInfo | null }[] {
    const fetched = ai?.catalog[provider];
    if (fetched && fetched.length) return fetched.map((info) => ({ id: info.id, info }));
    return (PROVIDERS.find((p) => p.id === provider)?.models ?? []).map((id) => ({ id, info: null }));
  }

  function optionText(id: string, info: ModelInfo | null, job: "item" | "guide") {
    if (!info || info.input_price == null) return id;
    const cost = job === "item" ? info.item_cost : info.guide_cost;
    return `${id} — ${money(info.input_price)} in / ${money(info.output_price)} out per 1M · ≈${money(cost)} per ${job}`;
  }

  function encode(r: ModelRef) {
    return `${r.provider}|${r.model}`;
  }

  function decode(v: string): ModelRef {
    const [provider, ...rest] = v.split("|");
    return { provider, model: rest.join("|") };
  }

  /** providers worth listing in the pickers: those with a key (the current pick is always kept) */
  function pickerProviders(current: ModelRef) {
    return PROVIDERS.filter((p) => hasKey(p.id) || p.id === current.provider);
  }

  function isListed(r: ModelRef) {
    return modelsOf(r.provider).some((m) => m.id === r.model);
  }

  async function refresh() {
    try {
      [builds, settings, character, profileDir, envKeys] = await Promise.all([
        invoke<BuildSummary[]>("list_builds"),
        invoke<Settings>("settings"),
        invoke<CharacterState>("get_character"),
        invoke<string>("profile_dir"),
        invoke<Record<string, boolean>>("env_keys"),
      ]);
      if (saveState === "saved" && settings) {
        ai = structuredClone($state.snapshot(settings.ai));
        showCustom = !!ai.providers[CUSTOM]?.api_key || !!ai.providers[CUSTOM]?.base_url;
      }
    } catch (err) {
      say(String(err), "error");
    }
  }

  async function activate(id: string) {
    try {
      builds = await invoke<BuildSummary[]>("activate_build", { id });
      character = await invoke<CharacterState>("get_character");
      say("active build changed");
    } catch (err) {
      say(String(err), "error");
    }
  }

  async function remove(b: BuildSummary) {
    if (!confirm(`Delete build "${b.build_name}" of ${b.character_name}? The stored guide and profile are removed.`)) return;
    try {
      builds = await invoke<BuildSummary[]>("delete_build", { id: b.id });
      character = await invoke<CharacterState>("get_character");
    } catch (err) {
      say(String(err), "error");
    }
  }

  function loadIntoForm(b: BuildSummary) {
    if (b.builtin) return;
    characterName = b.character_name;
    buildName = b.build_name;
    source = b.source === "pasted guide" ? "" : b.source;
  }

  async function analyze() {
    if (!ai) return;
    analysisResult = null;
    if (!characterName.trim() || !buildName.trim()) {
      analysisResult = { text: "Character name and build name are required.", kind: "error" };
      return;
    }
    if (guideText.trim().length < 200) {
      analysisResult = { text: "Paste the whole guide page (the text is too short).", kind: "error" };
      return;
    }
    if (!ai.guide.model || !hasKey(ai.guide.provider)) {
      analysisResult = { text: `No usable guide model: pick one under AI models and add the ${providerName(ai.guide.provider)} key.`, kind: "error" };
      return;
    }
    if (saveState !== "saved") await saveAi();
    analyzing = true;
    try {
      const outcome = await invoke<GuideOutcome>("analyze_guide", { characterName, buildName, guideText, source, force });
      analysisResult = outcome.reused
        ? { text: `Guide unchanged since the last analysis: the stored profile "${outcome.build.profile_name}" was reused and activated (no AI call).`, kind: "info" }
        : { text: `Analysed with ${refLabel(ai.guide)}: "${outcome.build.profile_name}" (${outcome.build.stat_count} stats, ${outcome.build.facts.length} facts). Now active. ${outcome.usage}`, kind: "info" };
      guideText = "";
      force = false;
      await refresh();
    } catch (err) {
      analysisResult = { text: String(err), kind: "error" };
    } finally {
      analyzing = false;
    }
  }

  // ---- character facts ----
  function scheduleCharacterSave() {
    if (charSaveTimer) clearTimeout(charSaveTimer);
    charSaveTimer = setTimeout(saveCharacter, 400);
  }

  async function saveCharacter() {
    if (!character) return;
    try {
      character = await invoke<CharacterState>("save_character", { character: $state.snapshot(character) });
    } catch (err) {
      say(String(err), "error");
    }
  }

  function setFlag(key: string, value: boolean) {
    if (!character) return;
    character.flags = { ...(character.flags ?? {}), [key]: value };
    scheduleCharacterSave();
  }

  function setCounter(key: string, value: number) {
    if (!character) return;
    character.counters = { ...(character.counters ?? {}), [key]: Math.max(0, Math.floor(value || 0)) };
    scheduleCharacterSave();
  }

  // ---- AI settings (autosaved) ----
  function queueSave() {
    saveState = "unsaved";
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(saveAi, 700);
  }

  async function saveAi() {
    if (!ai) return;
    if (saveTimer) clearTimeout(saveTimer);
    saveState = "saving";
    try {
      settings = await invoke<Settings>("save_ai_settings", { ai: $state.snapshot(ai) });
      ai = structuredClone($state.snapshot(settings.ai));
      saveState = "saved";
    } catch (err) {
      saveState = "unsaved";
      say(String(err), "error");
    }
  }

  function configFor(provider: string): ModelConfig {
    const p = ai?.providers[provider] ?? { api_key: "", base_url: "" };
    return { provider, model: "-", api_key: p.api_key, base_url: p.base_url, vision: true, effort: ai?.effort ?? "medium" };
  }

  /** Save the key, then fetch the provider's model list (which proves the key works). */
  async function connect(provider: string) {
    if (!ai) return;
    await saveAi();
    connecting = { ...connecting, [provider]: true };
    connectError = { ...connectError, [provider]: "" };
    try {
      const list = await invoke<ModelInfo[]>("list_models", { config: configFor(provider) });
      if (ai) ai.catalog = { ...ai.catalog, [provider]: list };
      say(`${providerName(provider)}: ${list.length} models available`);
    } catch (err) {
      connectError = { ...connectError, [provider]: String(err) };
    } finally {
      connecting = { ...connecting, [provider]: false };
    }
  }

  function keyChanged(provider: string) {
    if (!ai) return;
    if (ai.providers[provider]?.api_key.trim()) connect(provider);
    else queueSave();
  }

  function setJob(job: "item" | "guide", r: ModelRef) {
    if (!ai || !r.model.trim()) return;
    const clean = { provider: r.provider, model: r.model.trim() };
    if (job === "item") {
      ai.item = clean;
      if (!ai.cycle.some((c) => same(c, clean))) ai.cycle = [clean, ...ai.cycle];
    } else {
      ai.guide = clean;
    }
    queueSave();
  }

  function picked(job: "item" | "guide", value: string) {
    if (value === "custom") {
      customMode = { ...customMode, [job]: true };
      return;
    }
    customMode = { ...customMode, [job]: false };
    setJob(job, decode(value));
  }

  function cycleAdd(value: string) {
    if (!ai || !value) return;
    const r = decode(value);
    if (!ai.cycle.some((c) => same(c, r))) ai.cycle = [...ai.cycle, r];
    addToCycle = "";
    queueSave();
  }

  function cycleRemove(r: ModelRef) {
    if (!ai) return;
    if (same(r, ai.item)) {
      say("the current item model stays in the list; pick another item model first", "error");
      return;
    }
    ai.cycle = ai.cycle.filter((c) => !same(c, r));
    queueSave();
  }

  onMount(() => {
    refresh();
    const unlisten = [
      listen<void>("builds-opened", () => refresh()),
      listen<Settings>("settings-changed", (e) => {
        settings = e.payload;
        if (saveState === "saved") ai = structuredClone(e.payload.ai);
      }),
    ];
    return () => unlisten.forEach((p) => p.then((f) => f()));
  });
</script>

<div class="builds h-screen overflow-y-auto bg-le-ink text-[13px] text-zinc-200">
  <header class="sticky top-0 z-10 border-b border-le-gold/20 bg-le-ink/95 px-6 py-3 backdrop-blur">
    <div class="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1">
      <div>
        <h1 class="text-[17px] font-bold tracking-wide text-le-gold">Builds &amp; AI</h1>
        <div class="text-[11px] text-zinc-400">
          Active: <span class="text-zinc-200">{active?.profile_name ?? "…"}</span>
        </div>
      </div>
      {#if settings}
        <div class="text-[11px] text-zinc-400">
          <span class="kbd">{settings.hotkey}</span> scan ·
          <span class="kbd">{settings.ai.hotkey_ai}</span> AI verdict ·
          <span class="kbd">{settings.ai.hotkey_cycle}</span> switch AI model
        </div>
      {/if}
    </div>
  </header>

  <div class="grid gap-6 px-6 py-5 lg:grid-cols-[300px_minmax(0,1fr)]">
    <!-- builds list -->
    <aside class="space-y-3">
      <h2 class="section-title">Saved builds</h2>
      {#each builds as b (b.id)}
        <div class="panel {b.active ? 'border-le-gold/70' : ''}">
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0">
              <div class="truncate font-semibold text-zinc-100">{b.build_name}</div>
              <div class="truncate text-[11px] text-zinc-400">{b.character_name}</div>
            </div>
            {#if b.active}
              <span class="shrink-0 rounded-full bg-le-gold/20 px-2 py-px text-[10px] font-semibold uppercase tracking-wider text-le-gold">active</span>
            {/if}
          </div>
          <div class="mt-1 truncate text-[11px] text-zinc-500" title={b.profile_name}>{b.profile_name}</div>
          <div class="mt-1 text-[11px] text-zinc-500">{b.stat_count} stats · {b.model} · {when(b.updated)}</div>
          <div class="mt-2 flex gap-2">
            {#if !b.active}
              <button class="btn" onclick={() => activate(b.id)}>Use</button>
            {/if}
            {#if !b.builtin}
              <button class="btn" onclick={() => loadIntoForm(b)} title="Fill the form to re-paste an updated guide">Update guide</button>
              <button class="btn btn-danger ml-auto" onclick={() => remove(b)}>Delete</button>
            {/if}
          </div>
        </div>
      {/each}
      <p class="text-[11px] leading-relaxed text-zinc-500">
        Each build stores its pasted guide and the AI-generated stat priorities. Switching builds never calls the AI; only a new or changed guide does.
      </p>
    </aside>

    <main class="space-y-8">
      <!-- paste a guide -->
      <section class="space-y-3">
        <h2 class="section-title">Paste a build guide</h2>
        <div class="panel space-y-3">
          <div class="grid gap-3 sm:grid-cols-2">
            <label class="block">
              <span class="label">Character name</span>
              <input class="field" list="known-characters" bind:value={characterName} placeholder="e.g. Aurelia" />
              <datalist id="known-characters">
                {#each knownCharacters as c}<option value={c}></option>{/each}
              </datalist>
            </label>
            <label class="block">
              <span class="label">Build name</span>
              <input class="field" bind:value={buildName} placeholder="e.g. Maxroll Paladin leveling" />
            </label>
          </div>
          <label class="block">
            <span class="label">Source URL <span class="text-zinc-500">(optional)</span></span>
            <input class="field" bind:value={source} placeholder="https://maxroll.gg/last-epoch/build-guides/…" />
          </label>
          <label class="block">
            <span class="label">Guide text</span>
            <textarea class="field h-56 resize-y font-mono text-[12px]" bind:value={guideText}
              placeholder="Select the whole guide page in your browser (Ctrl+A, Ctrl+C) and paste it here. Stat priorities, gearing and skill sections are what matter."></textarea>
            <div class="mt-1 text-[11px] text-zinc-500">{guideText.trim().length.toLocaleString()} characters{#if matchingBuild} · this build exists: unchanged text reuses its profile, changed text re-analyses it{/if}</div>
          </label>
          <div class="flex flex-wrap items-center gap-3">
            <span class="text-[12px] text-zinc-400">Analyses with <span class="text-zinc-200">{refLabel(ai?.guide)}</span> <span class="text-zinc-600">(change under AI models)</span></span>
            <label class="flex items-center gap-2 text-[12px] text-zinc-400">
              <input type="checkbox" bind:checked={force} /> re-analyse even if unchanged
            </label>
            <button class="btn btn-primary ml-auto" disabled={analyzing} onclick={analyze}>
              {#if analyzing}<span class="spinner"></span> Analysing… (1–3 min){:else}Analyse with AI{/if}
            </button>
          </div>
          {#if analysisResult}
            <div class="rounded-md border px-3 py-2 text-[12px] leading-relaxed {analysisResult.kind === 'error' ? 'border-le-worse/40 bg-le-worse/10 text-le-worse' : 'border-le-upgrade/40 bg-le-upgrade/10 text-zinc-100'}">
              {analysisResult.text}
            </div>
          {/if}
        </div>
      </section>

      <!-- character facts -->
      <section class="space-y-3">
        <h2 class="section-title">Character</h2>
        {#if character && active}
          <div class="panel space-y-3">
            <div class="grid gap-3 sm:grid-cols-[auto_1fr] sm:items-center">
              <label class="flex items-center gap-2">
                <span class="label mb-0">Level</span>
                <input class="field w-20" type="number" min="1" max="100" bind:value={character.level} oninput={scheduleCharacterSave} />
              </label>
              <div class="text-[11px] text-zinc-400">
                Resistances (from the sheet, refresh with <span class="kbd">{settings?.hotkey}</span> over the character screen):
                {#each Object.entries(character.resistances) as [e, v]}
                  <span class="ml-1 whitespace-nowrap"><span class="capitalize">{e}</span> <span class={v >= 75 ? "text-le-upgrade" : "text-le-side"}>{Math.round(v)}%</span></span>
                {/each}
                · endurance {Math.round(character.endurance)}%
              </div>
            </div>
            <div class="h-px bg-le-gold/15"></div>
            <div class="text-[11px] text-zinc-400">Facts the active build's priorities depend on. The scorer cannot see these in tooltips, so keep them current (saved automatically).</div>
            {#if active.builtin}
              <div class="grid gap-2 sm:grid-cols-2">
                <label class="fact"><span>Heaven's Bulwark points</span>
                  <input class="field w-20" type="number" min="0" max="10" bind:value={character.heavens_bulwark_points} oninput={scheduleCharacterSave} /></label>
                <label class="fact"><span>Healing Hands specialised</span>
                  <input type="checkbox" bind:checked={character.healing_hands_specced} onchange={scheduleCharacterSave} /></label>
                <label class="fact"><span>Solarum Plate equipped</span>
                  <input type="checkbox" bind:checked={character.solarum_plate_equipped} onchange={scheduleCharacterSave} /></label>
                <label class="fact"><span>Nagasa Scymitar equipped</span>
                  <input type="checkbox" bind:checked={character.nagasa_scymitar_equipped} onchange={scheduleCharacterSave} /></label>
              </div>
            {:else if active.facts.length === 0}
              <div class="text-[12px] text-zinc-500">This build's priorities have no conditional facts.</div>
            {:else}
              <div class="grid gap-2 sm:grid-cols-2">
                {#each active.facts as f (f.key)}
                  <label class="fact" title={f.note}>
                    <span>{f.label}{#if f.note}<span class="block text-[11px] text-zinc-500">{f.note}</span>{/if}</span>
                    {#if f.kind === "counter"}
                      <input class="field w-20" type="number" min="0" value={character.counters?.[f.key] ?? f.default_count}
                        oninput={(e) => setCounter(f.key, Number((e.currentTarget as HTMLInputElement).value))} />
                    {:else}
                      <input type="checkbox" checked={character.flags?.[f.key] ?? f.default_on}
                        onchange={(e) => setFlag(f.key, (e.currentTarget as HTMLInputElement).checked)} />
                    {/if}
                  </label>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </section>

      <!-- AI -->
      <section class="space-y-3">
        <div class="flex items-baseline justify-between">
          <h2 class="section-title">AI models</h2>
          <div class="text-[11px] {saveState === 'saved' ? 'text-zinc-600' : 'text-le-side'}">
            {saveState === "saved" ? "saved" : saveState === "saving" ? "saving…" : "unsaved"}
          </div>
        </div>
        {#if ai}
          <!-- 1. providers -->
          <div class="panel space-y-2">
            <div class="text-[11px] text-zinc-400">1 · Add an API key for at least one provider. Keys are stored in <span class="font-mono">settings.json</span>; a key from the environment works too.</div>
            {#each PROVIDERS.filter((p) => p.id !== CUSTOM) as p (p.id)}
              <div class="grid items-center gap-x-3 gap-y-1 sm:grid-cols-[130px_minmax(0,1fr)_190px_auto]">
                <div class="font-medium text-zinc-100">{p.name}</div>
                <input class="field font-mono" type="password" autocomplete="off" placeholder={`paste key (or set ${p.env})`}
                  bind:value={ai.providers[p.id].api_key} oninput={queueSave} onchange={() => keyChanged(p.id)} />
                <div class="text-[11px]">
                  {#if connecting[p.id]}
                    <span class="text-zinc-400"><span class="spinner inline-block align-middle"></span> connecting…</span>
                  {:else if ai.catalog[p.id]?.length}
                    <span class="text-le-upgrade">●</span> {ai.catalog[p.id].length} models · {keyStatus(p.id)}
                  {:else if hasKey(p.id)}
                    <span class="text-le-side">●</span> {keyStatus(p.id)}, not checked
                  {:else}
                    <span class="text-zinc-600">○ no key</span>
                  {/if}
                </div>
                <button class="btn" disabled={connecting[p.id] || !hasKey(p.id)} onclick={() => connect(p.id)}>{ai.catalog[p.id]?.length ? "Refresh" : "Connect"}</button>
                {#if connectError[p.id]}
                  <div class="text-[11px] text-le-worse sm:col-span-4">{connectError[p.id]}</div>
                {/if}
              </div>
            {/each}
            <button class="text-[11px] text-zinc-500 hover:text-zinc-300" onclick={() => (showCustom = !showCustom)}>{showCustom ? "▾" : "▸"} Custom OpenAI-compatible server</button>
            {#if showCustom}
              <div class="grid items-center gap-x-3 gap-y-1 sm:grid-cols-[130px_minmax(0,1fr)_minmax(0,1fr)_auto]">
                <div class="font-medium text-zinc-100">Custom server</div>
                <input class="field font-mono" placeholder="base URL, e.g. http://localhost:11434/v1" bind:value={ai.providers[CUSTOM].base_url} oninput={queueSave} />
                <input class="field font-mono" type="password" autocomplete="off" placeholder="API key (if the server needs one)" bind:value={ai.providers[CUSTOM].api_key} oninput={queueSave} onchange={() => keyChanged(CUSTOM)} />
                <button class="btn" disabled={connecting[CUSTOM] || !ai.providers[CUSTOM].base_url.trim()} onclick={() => connect(CUSTOM)}>{ai.catalog[CUSTOM]?.length ? "Refresh" : "Connect"}</button>
                {#if connectError[CUSTOM]}<div class="text-[11px] text-le-worse sm:col-span-4">{connectError[CUSTOM]}</div>{/if}
              </div>
            {/if}
          </div>

          <!-- 2. model pickers -->
          <div class="panel space-y-3">
            <div class="text-[11px] text-zinc-400">2 · Choose a model per job. Prices are list prices per 1M tokens with a rough cost per call; Connect above fills in a provider's real model list.</div>
            {#each [{ job: "item", label: `Item verdicts (${ai.hotkey_ai})`, hint: "a fast, cheap model does well here", current: ai.item }, { job: "guide", label: "Guide analysis", hint: "use the most capable model; a guide is analysed once", current: ai.guide }] as row (row.job)}
              {@const job = row.job as "item" | "guide"}
              {@const listed = isListed(row.current) && !customMode[job]}
              <div class="grid items-center gap-x-3 gap-y-1 sm:grid-cols-[190px_minmax(0,1fr)]">
                <div>
                  <div class="font-medium text-zinc-100">{row.label}</div>
                  <div class="text-[11px] text-zinc-500">{row.hint}</div>
                </div>
                <div class="space-y-1">
                  <select class="field" value={listed ? encode(row.current) : "custom"} onchange={(e) => picked(job, (e.currentTarget as HTMLSelectElement).value)}>
                    {#each pickerProviders(row.current) as p (p.id)}
                      {#if modelsOf(p.id).length}
                        <optgroup label={p.name + (ai.catalog[p.id]?.length ? "" : " (suggested; Connect for the real list)")}>
                          {#each modelsOf(p.id) as m (m.id)}
                            <option value={encode({ provider: p.id, model: m.id })}>{optionText(m.id, m.info, job)}</option>
                          {/each}
                        </optgroup>
                      {/if}
                    {/each}
                    <option value="custom">Other model id…</option>
                  </select>
                  {#if !listed}
                    <div class="flex gap-2">
                      <select class="field w-auto" value={row.current.provider} onchange={(e) => setJob(job, { provider: (e.currentTarget as HTMLSelectElement).value, model: row.current.model })}>
                        {#each PROVIDERS as p}<option value={p.id}>{p.name}</option>{/each}
                      </select>
                      <input class="field font-mono" placeholder="model id, then Enter" value={row.current.model}
                        onchange={(e) => setJob(job, { provider: row.current.provider, model: (e.currentTarget as HTMLInputElement).value })} />
                    </div>
                  {/if}
                  {#if row.current.model && !hasKey(row.current.provider)}
                    <div class="text-[11px] text-le-worse">{providerName(row.current.provider)} has no key: this job cannot run yet.</div>
                  {/if}
                </div>
              </div>
            {/each}

            <div class="grid items-start gap-x-3 gap-y-1 sm:grid-cols-[190px_minmax(0,1fr)]">
              <div>
                <div class="font-medium text-zinc-100">{ai.hotkey_cycle} switches between</div>
                <div class="text-[11px] text-zinc-500">the item model rotates through this list</div>
              </div>
              <div class="flex flex-wrap items-center gap-1.5">
                {#each ai.cycle as r (encode(r))}
                  <span class="chip {same(r, ai.item) ? 'chip-active' : ''} {hasKey(r.provider) ? '' : 'opacity-50'}" title={hasKey(r.provider) ? "" : "no key for this provider: skipped"}>
                    {refLabel(r)}
                    <button class="ml-1 text-zinc-500 hover:text-le-worse" onclick={() => cycleRemove(r)} title="remove">✕</button>
                  </span>
                {/each}
                <select class="field w-auto py-1 text-[12px]" bind:value={addToCycle} onchange={(e) => cycleAdd((e.currentTarget as HTMLSelectElement).value)}>
                  <option value="">+ add…</option>
                  {#each PROVIDERS.filter((p) => hasKey(p.id)) as p (p.id)}
                    <optgroup label={p.name}>
                      {#each modelsOf(p.id).filter((m) => !ai!.cycle.some((c) => c.provider === p.id && c.model === m.id)) as m (m.id)}
                        <option value={encode({ provider: p.id, model: m.id })}>{m.id}</option>
                      {/each}
                    </optgroup>
                  {/each}
                </select>
              </div>
            </div>

            <button class="text-[11px] text-zinc-500 hover:text-zinc-300" onclick={() => (showAdvanced = !showAdvanced)}>{showAdvanced ? "▾" : "▸"} Advanced</button>
            {#if showAdvanced}
              <div class="grid items-center gap-x-3 gap-y-2 sm:grid-cols-[190px_minmax(0,1fr)]">
                <div class="font-medium text-zinc-100">Reasoning effort</div>
                <select class="field w-auto" bind:value={ai.effort} onchange={queueSave}>
                  <option value="low">low (fastest)</option><option value="medium">medium</option><option value="high">high (slow, thorough)</option>
                </select>
                <div class="text-[11px] leading-relaxed text-zinc-500 sm:col-span-2">
                  Hotkeys live in <span class="font-mono">{profileDir}\settings.json</span> (restart the overlay after changing them). Prices come from <span class="font-mono">model_prices.json</span>; copy it into the profile folder to add or correct entries. Text-only models get the OCR text instead of screenshots automatically. Only tooltip crops, OCR text and the pasted guide leave the machine, and only to the provider you chose.
                </div>
              </div>
            {/if}
          </div>
        {/if}
      </section>
    </main>
  </div>

  {#if toast}
    <div class="fixed bottom-4 right-4 rounded-lg border px-3 py-2 text-[12px] shadow-lg {toast.kind === 'error' ? 'border-le-worse/50 bg-[#2a1210] text-le-worse' : 'border-le-gold/40 bg-[#1a1510] text-zinc-200'}">
      {toast.text}
    </div>
  {/if}
</div>
