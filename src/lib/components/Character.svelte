<script lang="ts">
  import { characterState, type CharacterState } from '../stores/character';

  export let skinType: 'default-css' | 'rive' | 'lottie' = 'default-css';

  let state: CharacterState;
  $: state = $characterState;

  // will-change 只在动画活跃时开启，idle 时释放 GPU 合成层
  // 旧款 Intel Mac 显存有限，持续占用合成层会拖慢整机
  $: willChange = state !== 'idle' ? 'transform' : 'auto';
</script>

<div
  class="character state-{state}"
  style:will-change={willChange}

  role="img"
  aria-label="Pocket Agent 角色 — 当前状态: {state}"
>
  {#if skinType === 'default-css'}
    <div class="body-wrap">
      <div class="ear left"></div>
      <div class="ear right"></div>
      <div class="head">
        <div class="face">
          <div class="eyes">
            <div class="eye left"></div>
            <div class="eye right"></div>
          </div>
          <div class="mouth"></div>
        </div>
      </div>
      <div class="torso"></div>
    </div>
  {/if}
  <!-- 扩展口：skinType === 'rive' | 'lottie' 在 Phase 3 接入 -->
</div>

<style>
  /* ─── 布局 ─── */
  .character {
    width: 120px;
    height: 140px;
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    cursor: grab;
    user-select: none;
  }
  .character:active { cursor: grabbing; }

  .body-wrap {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  /* ─── 几何形状 ─── */
  .head {
    width: 84px;
    height: 84px;
    background: #7c9eff;
    border-radius: 50%;
    position: relative;
    z-index: 1;
  }

  .ear {
    width: 20px;
    height: 28px;
    background: #7c9eff;
    border-radius: 50% 50% 0 0;
    position: absolute;
    top: 12px;
    z-index: 0;
    transform-origin: bottom center;
  }
  .ear.left  { left: 14px; }
  .ear.right { right: 14px; }

  .face {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding-top: 6px;
  }

  .eyes {
    display: flex;
    gap: 16px;
  }

  .eye {
    width: 13px;
    height: 13px;
    background: #1a1a2e;
    border-radius: 50%;
    transform-origin: center;
  }

  .mouth {
    width: 32px;
    height: 9px;
    background: #1a1a2e;
    border-radius: 0 0 16px 16px;
    transform-origin: center;
  }

  .torso {
    width: 56px;
    height: 36px;
    background: #7c9eff;
    border-radius: 28px 28px 18px 18px;
    margin-top: 4px;
  }

  /* ─── 动画：只用 transform / opacity，不触发 layout，旧 GPU 安全 ─── */

  /* idle：轻微呼吸 + 眨眼 */
  @keyframes breathe {
    0%, 100% { transform: scaleY(1);    }
    50%       { transform: scaleY(1.03); }
  }
  @keyframes blink {
    0%, 88%, 100% { transform: scaleY(1);   }
    93%           { transform: scaleY(0.08); }
  }

  /* listening：耳朵上扬 */
  @keyframes ear-perk {
    0%, 100% { transform: translateY(0)   rotate(0deg);   }
    50%      { transform: translateY(-5px) rotate(-12deg); }
  }

  /* thinking：头部左右摇摆 */
  @keyframes think-sway {
    0%, 100% { transform: rotate(0deg);  }
    25%      { transform: rotate(-4deg); }
    75%      { transform: rotate(4deg);  }
  }

  /* speaking：嘴型开合 */
  @keyframes mouth-talk {
    0%, 100% { transform: scaleY(1);   }
    50%      { transform: scaleY(0.2); }
  }

  /* ── idle ── */
  .state-idle .head {
    animation: breathe 3.2s ease-in-out infinite;
  }
  .state-idle .eye {
    animation: blink 4.5s ease-in-out infinite;
  }
  /* 两眼错开，避免同时眨 */
  .state-idle .eye.right {
    animation-delay: 0.15s;
  }

  /* ── listening ── */
  .state-listening .ear {
    animation: ear-perk 0.9s ease-in-out infinite;
  }
  .state-listening .ear.right {
    animation-delay: 0.12s;
  }

  /* ── thinking ── */
  .state-thinking .head {
    animation: think-sway 1.6s ease-in-out infinite;
  }

  /* ── speaking ── */
  .state-speaking .mouth {
    animation: mouth-talk 0.28s ease-in-out infinite;
  }
</style>
