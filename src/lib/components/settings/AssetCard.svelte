<script lang="ts">
  let {
    title,
    src,
    builtin,
    hint,
    choose,
    remove,
    chooseText,
    removeText,
    builtinText,
  }: {
    title: string;
    src: string | null;
    /** src is the bundled preset: label it and offer no restore button. */
    builtin: boolean;
    hint: string;
    choose: () => void;
    remove: () => void;
    chooseText: string;
    removeText: string;
    builtinText: string;
  } = $props();
</script>

<div class="asset">
  <div class="preview">
    {#if src}<img {src} alt="" />{:else}<span>PA</span>{/if}
  </div>
  <h3>{title}</h3>
  <p>{builtin ? `${builtinText} · ${hint}` : hint}</p>
  <div>
    <button class="secondary" onclick={choose}>{chooseText}</button>
    {#if !builtin}<button class="ghost danger" onclick={remove}>{removeText}</button>{/if}
  </div>
</div>
