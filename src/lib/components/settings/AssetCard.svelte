<script lang="ts">
  let {
    title,
    src,
    hint,
    choose,
    remove,
    chooseText,
    removeText,
  }: {
    title: string;
    src: string | null;
    hint: string;
    choose: () => void;
    remove: () => void;
    chooseText: string;
    removeText: string;
  } = $props();
</script>

<div class="asset">
  <div class="preview">
    {#if src}<img {src} alt="" />{:else}<span>PA</span>{/if}
  </div>
  <h3>{title}</h3>
  <p>{hint}</p>
  <div>
    <button class="secondary" onclick={choose}>{chooseText}</button>
    {#if src}<button class="ghost danger" onclick={remove}>{removeText}</button>{/if}
  </div>
</div>
