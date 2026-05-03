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
}

const translations: Record<string, Strings> = {
  zh: {
    hint: '按下 fn 说话，再按一次结束，或在下方输入文字',
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
  },
  en: {
    hint: 'Press fn to talk, press again to stop, or type below',
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
  },
  ja: {
    hint: 'fnキーで話す、もう一度で終了、または下に入力',
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
  },
  ko: {
    hint: 'fn 키를 눌러 말하기, 다시 눌러 끝내기, 또는 아래에 입력',
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
  },
};

// Fallback to zh for unsupported languages
const TRANSLATIONS: Record<string, Strings> = { ...translations };

function langFromVoice(voice: string): LangKey {
  const code = voice.split('-')[0];
  if (code in TRANSLATIONS) return code as LangKey;
  return 'zh';
}

export function t(voice: string): Strings {
  const lang = langFromVoice(voice);
  return TRANSLATIONS[lang] ?? TRANSLATIONS.zh;
}

export type { Strings };
