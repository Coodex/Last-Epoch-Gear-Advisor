<script lang="ts">
  import Overlay from "./Overlay.svelte";
  import Builds from "./Builds.svelte";

  // The same bundle serves both windows: the click-through overlay ("main")
  // and the normal Builds & AI window, opened as index.html?window=builds.
  const isBuilds = new URLSearchParams(window.location.search).get("window") === "builds";
</script>

{#if isBuilds}
  <Builds />
{:else}
  <Overlay />
{/if}
