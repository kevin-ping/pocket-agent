// Simple i18n: UI language derived from the primary TTS voice setting

type LangKey = 'zh' | 'en' | 'ja' | 'ko' | 'fr' | 'de' | 'es';

interface Strings {
  hint: string;
  inputPlaceholder: string;
  inputBusy: string;
  settings: string;
  general: string;
  voice: string;
  apiUrl: string;
  volume: string;
  skin: string;
  primaryLang: string;
  aux1Lang: string;
  aux2Lang: string;
  audioFormat: string;
  fixedLang: string;
  avatar: string;
  clearChat: string;
  mute: string;
  unmute: string;
  exit: string;
  close: string;
  save: string;
  cancel: string;
  autoDetectHint: string;
  connection: string;
  appearance: string;
  defaultOption: string;
  upload: string;
  clickToUpload: string;
  supportedFormats: string;
  removeAvatar: string;
  defaultAvatarHint: string;
  none: string;
  wavLossless: string;
  mp3Compact: string;
  ariaSettings: string;
  ariaCloseSettings: string;
  ariaAvatar: string;
  ariaVolume: string;
  breakConfirmTitle: string;
  breakConfirmBreak: string;
  breakConfirmContinue: string;
  language: string;
  uiLang: string;
  uiLangAuto: string;
  hotkey: string;
  recordKey: string;
  capturingKey: string;
  applyingKey: string;
  doubleClickRecord: string;
  voiceOutput: string;
  animAvatar: string;
}

const translations: Record<string, Strings> = {
  zh: {
    hint: '按下 {key} 说话，再按一次结束，或在下方输入文字',
    inputPlaceholder: '输入消息…',
    inputBusy: '处理中…',
    settings: '设置',
    general: '通用',
    voice: '语音',
    apiUrl: '接口地址',
    volume: '音量',
    skin: '皮肤',
    primaryLang: '主语言',
    aux1Lang: '辅助语言 1',
    aux2Lang: '辅助语言 2',
    audioFormat: '音频格式',
    fixedLang: '固定',
    avatar: '头像',
    clearChat: '清空对话',
    mute: '静音',
    unmute: '取消静音',
    exit: '退出',
    close: '关闭',
    save: '保存',
    cancel: '取消',
    autoDetectHint: '自动检测回复语言，匹配对应声音',
    connection: '连接',
    appearance: '外观',
    defaultOption: '默认',
    upload: '上传',
    clickToUpload: '点击上传头像',
    supportedFormats: '支持 JPG、PNG、GIF、WebP',
    removeAvatar: '移除头像',
    defaultAvatarHint: '未设置时显示默认角色',
    none: '（无）',
    wavLossless: 'WAV（无损）',
    mp3Compact: 'MP3（体积小）',
    ariaSettings: '设置',
    ariaCloseSettings: '关闭设置',
    ariaAvatar: '头像',
    ariaVolume: '音量',
    breakConfirmTitle: '当前对话还在进行中，是否打断？',
    breakConfirmBreak: '打断',
    breakConfirmContinue: '继续等待',
    language: '语言',
    uiLang: '界面语言',
    uiLangAuto: '自动（跟随语音）',
    hotkey: '快捷键',
    recordKey: '录音键',
    capturingKey: '按下快捷键…',
    applyingKey: '按键更换中…',
    doubleClickRecord: '双击录音',
    voiceOutput: '语音输出',
    animAvatar: '动画头像（思考/说话）',
  },
  en: {
    hint: 'Press {key} to talk, press again to stop, or type below',
    inputPlaceholder: 'Type a message…',
    inputBusy: 'Processing…',
    settings: 'Settings',
    general: 'General',
    voice: 'Voice',
    apiUrl: 'API URL',
    volume: 'Volume',
    skin: 'Skin',
    primaryLang: 'Primary',
    aux1Lang: 'Auxiliary 1',
    aux2Lang: 'Auxiliary 2',
    audioFormat: 'Audio Format',
    fixedLang: 'Lock',
    avatar: 'Avatar',
    clearChat: 'Clear Chat',
    mute: 'Mute',
    unmute: 'Unmute',
    exit: 'Exit',
    close: 'Close',
    save: 'Save',
    cancel: 'Cancel',
    autoDetectHint: 'Auto-detect response language, match voice',
    connection: 'Connection',
    appearance: 'Appearance',
    defaultOption: 'Default',
    upload: 'Upload',
    clickToUpload: 'Click to upload avatar',
    supportedFormats: 'Supports JPG, PNG, GIF, WebP',
    removeAvatar: 'Remove Avatar',
    defaultAvatarHint: 'Shows default character when not set',
    none: '(None)',
    wavLossless: 'WAV (Lossless)',
    mp3Compact: 'MP3 (Compact)',
    ariaSettings: 'Settings',
    ariaCloseSettings: 'Close settings',
    ariaAvatar: 'Avatar',
    ariaVolume: 'Volume',
    breakConfirmTitle: 'A conversation is still in progress. Interrupt it?',
    breakConfirmBreak: 'Interrupt',
    breakConfirmContinue: 'Keep waiting',
    language: 'Language',
    uiLang: 'UI Language',
    uiLangAuto: 'Auto (from voice)',
    hotkey: 'Hotkey',
    recordKey: 'Record Key',
    capturingKey: 'Press a key…',
    applyingKey: 'Applying…',
    doubleClickRecord: 'Double-Click Record',
    voiceOutput: 'Voice Output',
    animAvatar: 'Animated Avatar (thinking/speaking)',
  },
  ja: {
    hint: '{key}キーで話す、もう一度で終了、または下に入力',
    inputPlaceholder: 'メッセージを入力…',
    inputBusy: '処理中…',
    settings: '設定',
    general: '一般',
    voice: '音声',
    apiUrl: 'API URL',
    volume: '音量',
    skin: 'スキン',
    primaryLang: 'メイン言語',
    aux1Lang: 'サブ言語 1',
    aux2Lang: 'サブ言語 2',
    audioFormat: '音声形式',
    fixedLang: '固定',
    avatar: 'アバター',
    clearChat: '履歴クリア',
    mute: 'ミュート',
    unmute: 'ミュート解除',
    exit: '終了',
    close: '閉じる',
    save: '保存',
    cancel: 'キャンセル',
    autoDetectHint: '応答言語を自動検出し、音声をマッチング',
    connection: '接続',
    appearance: '外観',
    defaultOption: 'デフォルト',
    upload: 'アップロード',
    clickToUpload: 'クリックしてアバターをアップロード',
    supportedFormats: 'JPG、PNG、GIF、WebP対応',
    removeAvatar: 'アバターを削除',
    defaultAvatarHint: '未設定時はデフォルトキャラクターを表示',
    none: '（なし）',
    wavLossless: 'WAV（ロスレス）',
    mp3Compact: 'MP3（コンパクト）',
    ariaSettings: '設定',
    ariaCloseSettings: '設定を閉じる',
    ariaAvatar: 'アバター',
    ariaVolume: '音量',
    breakConfirmTitle: '会話がまだ進行中です。中断しますか？',
    breakConfirmBreak: '中断',
    breakConfirmContinue: '待つ',
    language: '言語',
    uiLang: 'UI言語',
    uiLangAuto: '自動（音声から）',
    hotkey: 'ホットキー',
    recordKey: '録音キー',
    capturingKey: 'キーを押して…',
    applyingKey: '適用中…',
    doubleClickRecord: 'ダブルクリック録音',
    voiceOutput: '音声出力',
    animAvatar: 'アニメーションアバター（思考/発話）',
  },
  ko: {
    hint: '{key} 키를 눌러 말하기, 다시 눌러 끝내기, 또는 아래에 입력',
    inputPlaceholder: '메시지 입력…',
    inputBusy: '처리 중…',
    settings: '설정',
    general: '일반',
    voice: '음성',
    apiUrl: 'API URL',
    volume: '볼륨',
    skin: '스킨',
    primaryLang: '기본 언어',
    aux1Lang: '보조 언어 1',
    aux2Lang: '보조 언어 2',
    audioFormat: '오디오 형식',
    fixedLang: '고정',
    avatar: '아바타',
    clearChat: '대화 지우기',
    mute: '음소거',
    unmute: '음소거 해제',
    exit: '종료',
    close: '닫기',
    save: '저장',
    cancel: '취소',
    autoDetectHint: '응답 언어 자동 감지, 음성 매칭',
    connection: '연결',
    appearance: '외관',
    defaultOption: '기본',
    upload: '업로드',
    clickToUpload: '클릭하여 아바타 업로드',
    supportedFormats: 'JPG, PNG, GIF, WebP 지원',
    removeAvatar: '아바타 제거',
    defaultAvatarHint: '설정하지 않으면 기본 캐릭터 표시',
    none: '(없음)',
    wavLossless: 'WAV (무손실)',
    mp3Compact: 'MP3 (작은 크기)',
    ariaSettings: '설정',
    ariaCloseSettings: '설정 닫기',
    ariaAvatar: '아바타',
    ariaVolume: '볼륨',
    breakConfirmTitle: '대화가 아직 진행 중입니다. 중단하시겠습니까?',
    breakConfirmBreak: '중단',
    breakConfirmContinue: '계속 기다리기',
    language: '언어',
    uiLang: 'UI 언어',
    uiLangAuto: '자동 (음성에서)',
    hotkey: '단축키',
    recordKey: '녹음 키',
    capturingKey: '키를 누르세요…',
    applyingKey: '적용 중…',
    doubleClickRecord: '더블클릭 녹음',
    voiceOutput: '음성 출력',
    animAvatar: '애니메이션 아바타 (생각/말하기)',
  },
};

// Fallback to zh for unsupported languages
const TRANSLATIONS: Record<string, Strings> = { ...translations };

export function langFromVoice(voice: string): LangKey {
  const code = voice.split('-')[0];
  if (code in TRANSLATIONS) return code as LangKey;
  return 'zh';
}

export function t(voice: string): Strings {
  const lang = langFromVoice(voice);
  return TRANSLATIONS[lang] ?? TRANSLATIONS.zh;
}

/** Resolve effective UI lang: explicit ui_lang overrides voice-derived lang. */
export function resolveLang(uiLang: string, voice: string): LangKey {
  if (uiLang && uiLang in TRANSLATIONS) return uiLang as LangKey;
  return langFromVoice(voice);
}

/** Translate by explicit lang key (used with settings.ui_lang). */
export function tLang(lang: string): Strings {
  const lk = (lang && lang in TRANSLATIONS) ? lang as LangKey : 'en';
  return TRANSLATIONS[lk] ?? TRANSLATIONS.zh;
}

/** Conversation labels by explicit lang key. */
export function convLabelsLang(lang: string): ConversationLabels {
  const lk = (lang && lang in CONVERSATION_LABELS) ? lang as LangKey : 'zh';
  return CONVERSATION_LABELS[lk] ?? CONVERSATION_LABELS.zh;
}

// ── Continuous-conversation UI labels (per LangKey; covers all 7 supported) ──
export interface ConversationLabels {
  continuousMode: string;
  silenceTimeout: string;
  skipInterruptConfirm: string;
  silenceSecondsSuffix: (n: number) => string;
  pauseTolerance: string;
  pauseToleranceMsSuffix: (n: number) => string;
  micSensitivity: string;
  micSensitivitySuffix: (v: number) => string;
  bargeInSensitivity: string;
  bargeInSensitivitySuffix: (v: number) => string;
  bargeInToggle: string;
  bargeInHint: string;
  voiceListening: string;
  /** First-launch venv bootstrap header, e.g. "🛠 语音环境准备中…" */
  voiceSetupInstalling: string;
  /** Per-phase label for setup detail */
  voiceSetupPhase: (phase: string) => string;
  /** Toast shown when user presses hotkey while venv install is still running */
  voiceSetupNotReady: string;
  /** Setup failed message prefix */
  voiceSetupError: string;
  // ── Phase B + C: wake word + wake-word sample verification (ENH-C5) ──
  wakeWordLabel: string;
  wakeWordHint: string;
  wakeWordThreshold: string;
  wakeWordModel: string;
  speakerVerificationLabel: string;
  speakerVerificationHint: string;
  enrollSpeakerButton: string;
  enrollRecordingCountdown: (secondsLeft: number) => string;
  enrollPhraseHint: string;
  enrollRenameAction: (name: string) => string;
  enrollSuccess: (confidence: number) => string;
  enrollFailedTooQuiet: (rmsDbfs?: number) => string;
  speakerRejectedToast: string;
  waitingForWakeCaption: string;
  verifyingSpeakerCaption: string;
}

function fmtSeconds(ms: number): string {
  return (ms / 1000).toFixed(1);
}

export const CONVERSATION_LABELS: Record<LangKey, ConversationLabels> = {
  zh: {
    continuousMode: '连续对话模式',
    silenceTimeout: '静默退出秒数',
    skipInterruptConfirm: '打断时跳过确认',
    silenceSecondsSuffix: (n) => `${n}秒`,
    pauseTolerance: '停顿容忍度',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}秒`,
    micSensitivity: '麦克风灵敏度',
    micSensitivitySuffix: (v) => v <= 0.008 ? '高(离麦克风远)' : v <= 0.014 ? '中' : '低(离麦克风近/嘈杂)',
    bargeInSensitivity: '打断灵敏度',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? '高(容易打断)' : v <= 0.06 ? '中' : '低(不易打断)'),
    bargeInToggle: '允许打断回答',
    bargeInHint: '麦克风听到持续声音就会打断AI回复。数值越大，需要越大声才能打断。笔记本内置麦克风建议 0.04；台式机/外接音箱建议 0.06~0.08；如果频繁误触发请调高。',
    voiceListening: '听着…',
    voiceSetupInstalling: '🛠 语音环境准备中（首次启动约 3-10 分钟）',
    voiceSetupPhase: (p) => p === 'creating-venv' ? '创建虚拟环境' : p === 'upgrading-pip' ? '升级 pip' : p === 'installing-deps' ? '下载安装依赖' : p,
    voiceSetupNotReady: '语音环境准备中，请稍候…',
    voiceSetupError: '⚠ 语音环境安装失败',
    wakeWordLabel: '唤醒词激活',
    wakeWordHint: '说 "hey jarvis" 开启对话',
    wakeWordThreshold: '唤醒灵敏度',
    wakeWordModel: '唤醒模型',
    speakerVerificationLabel: '唤醒词录制',
    speakerVerificationHint: '录制唤醒词样本，仅匹配样本的声音才能唤醒',
    enrollSpeakerButton: '录制唤醒词',
    enrollRecordingCountdown: (s) => `请说话…（剩余 ${s}s）`,
    enrollPhraseHint: '说你的唤醒词即可，例如："Hey Jarvis"',
    enrollRenameAction: (n) => `当前样本：${n} · 改名`,
    enrollSuccess: (c) => `唤醒词样本保存成功，相似度 ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `声音太轻（${d.toFixed(1)} dBFS，需 ≥ -45）— 请靠近麦克风或提高系统输入电平`
      : '声音太轻，请靠近麦克风再试',
    speakerRejectedToast: '唤醒词不匹配',
    waitingForWakeCaption: '等待唤醒词…',
    verifyingSpeakerCaption: '验证唤醒词中…',
  },
  en: {
    continuousMode: 'Continuous Conversation',
    silenceTimeout: 'Silence Timeout',
    skipInterruptConfirm: 'Skip Interrupt Confirmation',
    silenceSecondsSuffix: (n) => `${n}s`,
    pauseTolerance: 'Pause Tolerance',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}s`,
    micSensitivity: 'Mic Sensitivity',
    micSensitivitySuffix: (v) => v <= 0.008 ? 'High (far from mic)' : v <= 0.014 ? 'Medium' : 'Low (close/noisy)',
    bargeInSensitivity: 'Interrupt Sensitivity',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? 'High (easy)' : v <= 0.06 ? 'Medium' : 'Low (hard)'),
    bargeInToggle: 'Allow Interrupting Response',
    bargeInHint: 'Sustained sound above this threshold interrupts AI speech. Higher = harder to interrupt. Laptop built-in mic: 0.04. Desktop / external speakers: 0.06–0.08. Raise if false triggers occur.',
    voiceListening: 'Listening…',
    voiceSetupInstalling: '🛠 Setting up voice environment (first launch, 3-10 min)',
    voiceSetupPhase: (p) => p === 'creating-venv' ? 'creating virtualenv' : p === 'upgrading-pip' ? 'upgrading pip' : p === 'installing-deps' ? 'downloading dependencies' : p,
    voiceSetupNotReady: 'Voice setup in progress, please wait…',
    voiceSetupError: '⚠ Voice environment setup failed',
    wakeWordLabel: 'Wake Word',
    wakeWordHint: 'Say "hey jarvis" to start a conversation',
    wakeWordThreshold: 'Wake Sensitivity',
    wakeWordModel: 'Wake Model',
    speakerVerificationLabel: 'Wake Word Sample',
    speakerVerificationHint: 'Record a sample — only matching voice can trigger wake',
    enrollSpeakerButton: 'Record Wake Word',
    enrollRecordingCountdown: (s) => `Speak now… (${s}s left)`,
    enrollPhraseHint: 'Say your wake word — e.g. "Hey Jarvis"',
    enrollRenameAction: (n) => `Sample: ${n} · rename`,
    enrollSuccess: (c) => `Sample saved — similarity ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `Too quiet (${d.toFixed(1)} dBFS, need ≥ -45) — move closer or raise system input level`
      : 'Too quiet — move closer to the microphone and try again',
    speakerRejectedToast: 'Wake word not matched',
    waitingForWakeCaption: 'Waiting for wake word…',
    verifyingSpeakerCaption: 'Verifying wake word…',
  },
  ja: {
    continuousMode: '連続会話モード',
    silenceTimeout: '無音タイムアウト',
    skipInterruptConfirm: '中断時に確認をスキップ',
    silenceSecondsSuffix: (n) => `${n}秒`,
    pauseTolerance: '間の許容時間',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}秒`,
    micSensitivity: 'マイク感度',
    micSensitivitySuffix: (v) => v <= 0.008 ? '高（マイクから遠い）' : v <= 0.014 ? '中' : '低（近い/騒がしい）',
    bargeInSensitivity: '割り込み感度',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? '高（割り込みやすい）' : v <= 0.06 ? '中' : '低（割り込みにくい）'),
    bargeInToggle: '割り込み許可',
    bargeInHint: 'マイクが一定以上の音を検出するとAIの発話を中断します。数値が大きいほど中断しにくくなります。ノートPC内蔵マイク：0.04、デスクトップ/外部スピーカー：0.06〜0.08。誤検出が多い場合は上げてください。',
    voiceListening: '聞いています…',
    voiceSetupInstalling: '🛠 音声環境を準備中（初回起動：3-10 分）',
    voiceSetupPhase: (p) => p === 'creating-venv' ? '仮想環境を作成中' : p === 'upgrading-pip' ? 'pip をアップグレード中' : p === 'installing-deps' ? '依存関係をダウンロード中' : p,
    voiceSetupNotReady: '音声環境を準備中です。お待ちください…',
    voiceSetupError: '⚠ 音声環境のセットアップに失敗',
    wakeWordLabel: 'ウェイクワード',
    wakeWordHint: '「ヘイ ジャービス」と話すと会話開始',
    wakeWordThreshold: 'ウェイク感度',
    wakeWordModel: 'ウェイクモデル',
    speakerVerificationLabel: 'ウェイクワード録音',
    speakerVerificationHint: 'サンプルを録音 — 一致する声のみウェイク可',
    enrollSpeakerButton: 'ウェイクワードを録音',
    enrollRecordingCountdown: (s) => `話してください…（残り ${s}s）`,
    enrollPhraseHint: 'ウェイクワードを言ってください。例：「Hey Jarvis」',
    enrollRenameAction: (n) => `サンプル：${n} · 名前変更`,
    enrollSuccess: (c) => `サンプル保存完了 — 類似度 ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `音量が小さすぎます（${d.toFixed(1)} dBFS、≥ -45 が必要）— マイクに近づくか入力レベルを上げてください`
      : '音量が小さすぎます。マイクに近づけて再試行してください',
    speakerRejectedToast: 'ウェイクワード不一致',
    waitingForWakeCaption: 'ウェイクワード待機中…',
    verifyingSpeakerCaption: 'ウェイクワード照合中…',
  },
  ko: {
    continuousMode: '연속 대화 모드',
    silenceTimeout: '무음 타임아웃',
    skipInterruptConfirm: '중단 시 확인 건너뛰기',
    silenceSecondsSuffix: (n) => `${n}초`,
    pauseTolerance: '일시 정지 허용',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}초`,
    micSensitivity: '마이크 감도',
    micSensitivitySuffix: (v) => v <= 0.008 ? '높음 (마이크에서 멀리)' : v <= 0.014 ? '중간' : '낮음 (가깝거나 시끄러움)',
    bargeInSensitivity: '인터럽트 감도',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? '높음 (쉽게)' : v <= 0.06 ? '중간' : '낮음 (어려움)'),
    bargeInToggle: '인터럽트 허용',
    bargeInHint: '마이크가 일정 이상의 소리를 감지하면 AI 응답을 중단합니다. 숫자가 클수록 중단하기 어렵습니다. 노트북 내장 마이크: 0.04, 데스크톱/외부 스피커: 0.06~0.08. 오탐지가 잦으면 값을 올리세요.',
    voiceListening: '듣는 중…',
    voiceSetupInstalling: '🛠 음성 환경 준비 중 (첫 실행, 3-10분)',
    voiceSetupPhase: (p) => p === 'creating-venv' ? '가상 환경 생성 중' : p === 'upgrading-pip' ? 'pip 업그레이드 중' : p === 'installing-deps' ? '의존성 다운로드 중' : p,
    voiceSetupNotReady: '음성 환경을 준비 중입니다. 잠시 기다려주세요…',
    voiceSetupError: '⚠ 음성 환경 설치 실패',
    wakeWordLabel: '웨이크 워드',
    wakeWordHint: '"헤이 자비스"라고 말해서 대화를 시작하세요',
    wakeWordThreshold: '웨이크 감도',
    wakeWordModel: '웨이크 모델',
    speakerVerificationLabel: '화자 인증',
    speakerVerificationHint: '등록된 음성만 웨이크할 수 있음',
    enrollSpeakerButton: '음성 등록',
    enrollRecordingCountdown: (s) => `말씀해 주세요…（남은 시간 ${s}s）`,
    enrollPhraseHint: '한 문장만 말해 주세요. 예: "안녕하세요, 오늘 날씨가 좋네요"',
    enrollRenameAction: (n) => `현재: ${n} · 이름 변경`,
    enrollSuccess: (c) => `등록 완료 — 신뢰도 ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `소리가 너무 작습니다 (${d.toFixed(1)} dBFS, ≥ -45 필요) — 마이크에 가까이 가거나 입력 레벨을 높이세요`
      : '소리가 너무 작습니다. 마이크에 더 가까이서 다시 시도하세요',
    speakerRejectedToast: '인식되지 않은 화자',
    waitingForWakeCaption: '웨이크 워드 대기 중…',
    verifyingSpeakerCaption: '화자 인증 중…',
  },
  fr: {
    continuousMode: 'Conversation continue',
    silenceTimeout: 'Délai de silence',
    skipInterruptConfirm: "Ignorer la confirmation d'interruption",
    silenceSecondsSuffix: (n) => `${n}s`,
    pauseTolerance: 'Tolérance de pause',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}s`,
    micSensitivity: 'Sensibilité du micro',
    micSensitivitySuffix: (v) => v <= 0.008 ? 'Élevée (loin du micro)' : v <= 0.014 ? 'Moyenne' : 'Faible (proche/bruyant)',
    bargeInSensitivity: 'Sensibilité d\'interruption',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? 'Élevée (facile d\'interrompre)' : v <= 0.06 ? 'Moyenne' : 'Faible (difficile d\'interrompre)'),
    bargeInToggle: 'Autoriser l\'interruption',
    bargeInHint: 'Un son soutenu au-dessus de ce seuil interrompt la réponse IA. Plus élevé = plus difficile d\'interrompre. Micro intégré : 0.04. Enceintes externes : 0.06–0.08. Augmentez en cas de faux déclenchements.',
    voiceListening: 'À l\'écoute…',
    voiceSetupInstalling: '🛠 Préparation de l\'environnement vocal (premier lancement, 3-10 min)',
    voiceSetupPhase: (p) => p === 'creating-venv' ? 'création de l\'environnement virtuel' : p === 'upgrading-pip' ? 'mise à jour de pip' : p === 'installing-deps' ? 'téléchargement des dépendances' : p,
    voiceSetupNotReady: 'Préparation en cours, veuillez patienter…',
    voiceSetupError: '⚠ Échec de l\'installation de l\'environnement vocal',
    wakeWordLabel: 'Mot de réveil',
    wakeWordHint: 'Dites "hey jarvis" pour démarrer une conversation',
    wakeWordThreshold: 'Sensibilité du réveil',
    wakeWordModel: 'Modèle de réveil',
    speakerVerificationLabel: 'Vérification du locuteur',
    speakerVerificationHint: 'Seules les voix enregistrées peuvent réveiller',
    enrollSpeakerButton: 'Enregistrer la voix',
    enrollRecordingCountdown: (s) => `Parlez maintenant… (${s}s restantes)`,
    enrollPhraseHint: 'Dites une phrase, par exemple : « Bonjour, il fait beau aujourd\u2019hui »',
    enrollRenameAction: (n) => `Actuel : ${n} · renommer`,
    enrollSuccess: (c) => `Enregistré — confiance ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `Trop faible (${d.toFixed(1)} dBFS, ≥ -45 requis) — rapprochez-vous du micro ou augmentez le niveau d\u2019entrée`
      : 'Trop faible — rapprochez-vous du micro et réessayez',
    speakerRejectedToast: 'Locuteur non reconnu',
    waitingForWakeCaption: 'En attente du mot de réveil…',
    verifyingSpeakerCaption: 'Vérification du locuteur…',
  },
  de: {
    continuousMode: 'Fortlaufendes Gespräch',
    silenceTimeout: 'Stille-Timeout',
    skipInterruptConfirm: 'Unterbrechungsbestätigung überspringen',
    silenceSecondsSuffix: (n) => `${n}s`,
    pauseTolerance: 'Pausen-Toleranz',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}s`,
    micSensitivity: 'Mikrofonempfindlichkeit',
    micSensitivitySuffix: (v) => v <= 0.008 ? 'Hoch (weit vom Mikrofon)' : v <= 0.014 ? 'Mittel' : 'Niedrig (nah/laut)',
    bargeInSensitivity: 'Unterbrechungsempfindlichkeit',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? 'Hoch (leicht zu unterbrechen)' : v <= 0.06 ? 'Mittel' : 'Niedrig (schwer zu unterbrechen)'),
    bargeInToggle: 'Unterbrechung erlauben',
    bargeInHint: 'Anhaltender Schall über diesem Schwellenwert unterbricht die AI-Antwort. Höher = schwerer zu unterbrechen. Laptop-Mikro: 0.04. Desktop/externe Lautsprecher: 0.06–0.08. Bei Fehlauslösungen erhöhen.',
    voiceListening: 'Höre zu…',
    voiceSetupInstalling: '🛠 Sprachumgebung wird eingerichtet (Erststart, 3-10 Min)',
    voiceSetupPhase: (p) => p === 'creating-venv' ? 'erstelle virtuelle Umgebung' : p === 'upgrading-pip' ? 'aktualisiere pip' : p === 'installing-deps' ? 'lade Abhängigkeiten' : p,
    voiceSetupNotReady: 'Sprachumgebung wird vorbereitet, bitte warten…',
    voiceSetupError: '⚠ Einrichtung der Sprachumgebung fehlgeschlagen',
    wakeWordLabel: 'Aktivierungswort',
    wakeWordHint: 'Sage "hey jarvis", um ein Gespräch zu beginnen',
    wakeWordThreshold: 'Aktivierungsempfindlichkeit',
    wakeWordModel: 'Aktivierungsmodell',
    speakerVerificationLabel: 'Sprecherverifizierung',
    speakerVerificationHint: 'Nur registrierte Stimmen können aktivieren',
    enrollSpeakerButton: 'Stimme registrieren',
    enrollRecordingCountdown: (s) => `Sprich jetzt… (noch ${s}s)`,
    enrollPhraseHint: 'Sag einen Satz, z. B.: „Hallo, das Wetter ist schön heute"',
    enrollRenameAction: (n) => `Aktuell: ${n} · umbenennen`,
    enrollSuccess: (c) => `Registriert — Konfidenz ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `Zu leise (${d.toFixed(1)} dBFS, ≥ -45 nötig) — näher ans Mikrofon oder Eingangspegel erhöhen`
      : 'Zu leise — näher an das Mikrofon und erneut versuchen',
    speakerRejectedToast: 'Sprecher nicht erkannt',
    waitingForWakeCaption: 'Warte auf Aktivierungswort…',
    verifyingSpeakerCaption: 'Verifiziere Sprecher…',
  },
  es: {
    continuousMode: 'Conversación continua',
    silenceTimeout: 'Tiempo de silencio',
    skipInterruptConfirm: 'Omitir confirmación de interrupción',
    silenceSecondsSuffix: (n) => `${n}s`,
    pauseTolerance: 'Tolerancia de pausa',
    pauseToleranceMsSuffix: (ms) => `${fmtSeconds(ms)}s`,
    micSensitivity: 'Sensibilidad del micrófono',
    micSensitivitySuffix: (v) => v <= 0.008 ? 'Alta (lejos del micrófono)' : v <= 0.014 ? 'Media' : 'Baja (cerca/ruidoso)',
    bargeInSensitivity: 'Sensibilidad de interrupción',
    bargeInSensitivitySuffix: (v) => v.toFixed(2) + ' ' + (v <= 0.03 ? 'Alta (fácil interrumpir)' : v <= 0.06 ? 'Media' : 'Baja (difícil interrumpir)'),
    bargeInToggle: 'Permitir interrumpir',
    bargeInHint: 'Un sonido sostenido por encima de este umbral interrumpe la respuesta de la IA. Más alto = más difícil interrumpir. Micrófono integrado: 0.04. Altavoces externos: 0.06–0.08. Aumente si hay falsos disparos.',
    voiceListening: 'Escuchando…',
    voiceSetupInstalling: '🛠 Configurando entorno de voz (primer arranque, 3-10 min)',
    voiceSetupPhase: (p) => p === 'creating-venv' ? 'creando entorno virtual' : p === 'upgrading-pip' ? 'actualizando pip' : p === 'installing-deps' ? 'descargando dependencias' : p,
    voiceSetupNotReady: 'Entorno de voz en preparación, por favor espera…',
    voiceSetupError: '⚠ Error al configurar el entorno de voz',
    wakeWordLabel: 'Palabra de activación',
    wakeWordHint: 'Di "hey jarvis" para iniciar una conversación',
    wakeWordThreshold: 'Sensibilidad de activación',
    wakeWordModel: 'Modelo de activación',
    speakerVerificationLabel: 'Verificación de hablante',
    speakerVerificationHint: 'Solo voces registradas pueden activar',
    enrollSpeakerButton: 'Registrar voz',
    enrollRecordingCountdown: (s) => `Habla ahora… (${s}s restantes)`,
    enrollPhraseHint: 'Di una frase, por ejemplo: «Hola, hoy hace buen tiempo»',
    enrollRenameAction: (n) => `Actual: ${n} · renombrar`,
    enrollSuccess: (c) => `Registrado — confianza ${(c * 100).toFixed(0)}%`,
    enrollFailedTooQuiet: (d) => d !== undefined
      ? `Demasiado bajo (${d.toFixed(1)} dBFS, se requiere ≥ -45) — acércate o sube el nivel de entrada`
      : 'Demasiado bajo — acércate al micrófono y reintenta',
    speakerRejectedToast: 'Hablante no reconocido',
    waitingForWakeCaption: 'Esperando palabra de activación…',
    verifyingSpeakerCaption: 'Verificando hablante…',
  },
};

export function convLabels(voice: string): ConversationLabels {
  return CONVERSATION_LABELS[langFromVoice(voice)] ?? CONVERSATION_LABELS.zh;
}

// ── Status TTS phrases (spoken; intentionally short and natural) ──
export const STATUS_PHRASES: Record<LangKey, { thinking: string; querying: (n: string) => string; executing: string; runningCommand: string }> = {
  zh: { thinking: '正在思考',  querying: (n) => `查询 ${n}`,            executing: '正在执行操作',    runningCommand: '运行命令' },
  en: { thinking: 'Thinking', querying: (n) => `Calling ${n}`,         executing: 'Working on it',  runningCommand: 'Running command' },
  ja: { thinking: '考え中',    querying: (n) => `${n} を呼び出し中`,   executing: '実行中',          runningCommand: 'コマンド実行中' },
  ko: { thinking: '생각 중',   querying: (n) => `${n} 호출 중`,         executing: '실행 중',         runningCommand: '명령 실행 중' },
  fr: { thinking: 'Réflexion', querying: (n) => `Appel de ${n}`,        executing: 'En cours',       runningCommand: 'Exécution de la commande' },
  de: { thinking: 'Denke nach', querying: (n) => `${n} wird aufgerufen`, executing: 'In Arbeit',     runningCommand: 'Befehl wird ausgeführt' },
  es: { thinking: 'Pensando',  querying: (n) => `Llamando a ${n}`,      executing: 'Procesando',     runningCommand: 'Ejecutando comando' },
};

/** Detect dominant language of a short text via Unicode ranges. */
export function detectLang(text: string): LangKey {
  let zh = 0, ja = 0, ko = 0, en = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    if (code >= 0x3040 && code <= 0x30FF) ja++;
    else if (code >= 0xAC00 && code <= 0xD7AF) ko++;
    else if (code >= 0x4E00 && code <= 0x9FFF) zh++;
    else if ((code >= 0x41 && code <= 0x5A) || (code >= 0x61 && code <= 0x7A)) en++;
  }
  if (ja > 0) return 'ja';
  if (ko > 0) return 'ko';
  if (zh > en && zh > 0) return 'zh';
  if (en > 0) return 'en';
  return 'zh';
}

export type { Strings, LangKey };
