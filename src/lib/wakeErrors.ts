// Wake word / voiceprint errors arrive from the backend as a JSON string
// {"wake_error": code, "params": {...}} (src-tauri/src/voice/sherpa_wake.rs).
// Users see a localized "problem + what to do"; raw details stay in the log.
import { convLabelsLang } from './i18n';
import { SETTINGS_LANGS, type SettingsLang } from './settingsI18n';

export const WAKE_ERROR_CODES = [
  'wake_phrase_empty', 'wake_phrase_invalid', 'wake_phrase_unpronounceable',
  'wake_engine_unsupported_os', 'wake_engine_unavailable', 'wake_model_missing',
  'voiceprint_missing', 'voiceprint_enroll_failed', 'wake_mic_unavailable',
  'wake_service_unavailable', 'wake_service_error',
] as const;
export type WakeErrorCode = typeof WAKE_ERROR_CODES[number];
export type WakeError = { code: WakeErrorCode; params: Record<string, string> };

type P = Record<string, string>;
type WakeErrorStrings =
  // The empty-phrase text is shared with the main window (convLabels.wakeNeedsPhrase).
  Record<Exclude<WakeErrorCode, 'wake_phrase_empty'>, (p: P) => string> & {
    saved: string;
    savedButWakeOff: (reason: string) => string;
  };

const WAKE_ERROR_TEXT: Record<SettingsLang, WakeErrorStrings> = {
  zh: {
    wake_phrase_invalid: () => '唤醒词格式不对：请只用 2–48 个汉字、英文字母和空格。',
    wake_phrase_unpronounceable: (p) => `无法识别「${p.word}」的发音：请换成常见的英文单词，或改用中文唤醒词。`,
    wake_engine_unsupported_os: (p) => `唤醒和声纹暂不可用：当前 macOS ${p.current} 低于唤醒引擎需要的 ${p.required}。PA 启动时会自动安装兼容组件；如果仍看到这条提示，请联网后重启 PA，或升级到 macOS ${p.required} 及以上。`,
    wake_engine_unavailable: () => '唤醒引擎加载失败，唤醒和声纹暂不可用：请重启 PA；如果仍然出现，请查看日志。',
    wake_model_missing: () => '唤醒模型还没下载：请在「唤醒词」设置里下载模型。',
    voiceprint_missing: () => '还没有声纹：请先录制声纹，或关闭「仅本人唤醒」。',
    voiceprint_enroll_failed: () => '声纹录制失败：请在安静的环境里重新录制，并说满 5 秒。',
    wake_mic_unavailable: () => '无法打开麦克风：请在「系统设置 → 隐私与安全性 → 麦克风」中允许 Pocket Agent。',
    wake_service_unavailable: () => '语音服务还没准备好：请稍等几秒再试；如果一直这样，请重启 PA。',
    wake_service_error: () => '唤醒服务出错：请重启 PA；如果仍然出现，请查看日志。',
    saved: '设置已保存。',
    savedButWakeOff: (r) => `设置已保存，但唤醒未启动：${r}`,
  },
  en: {
    wake_phrase_invalid: () => 'Invalid wake phrase: use 2–48 Chinese characters, English letters and spaces.',
    wake_phrase_unpronounceable: (p) => `No pronunciation found for "${p.word}": use a common English word or a Chinese wake phrase.`,
    wake_engine_unsupported_os: (p) => `Wake word and voiceprint are unavailable: macOS ${p.current} is older than the ${p.required} the wake engine needs. PA installs a compatible component at startup; if you still see this, connect to the internet and restart PA, or update to macOS ${p.required} or later.`,
    wake_engine_unavailable: () => 'The wake engine failed to load, so wake word and voiceprint are unavailable: restart PA; if this persists, check the log.',
    wake_model_missing: () => 'The wake model is not downloaded: download it in Wake word settings.',
    voiceprint_missing: () => 'No voiceprint yet: record your voice first, or turn off "Only my voice".',
    voiceprint_enroll_failed: () => 'Voiceprint recording failed: record again somewhere quiet and speak for the full 5 seconds.',
    wake_mic_unavailable: () => 'Cannot open the microphone: allow Pocket Agent in System Settings → Privacy & Security → Microphone.',
    wake_service_unavailable: () => 'The voice service is not ready yet: wait a few seconds and try again; if it keeps happening, restart PA.',
    wake_service_error: () => 'The wake service hit an error: restart PA; if this persists, check the log.',
    saved: 'Settings saved.',
    savedButWakeOff: (r) => `Settings saved, but the wake word did not start: ${r}`,
  },
  ja: {
    wake_phrase_invalid: () => 'ウェイクワードの形式が正しくありません：漢字・英字・スペースで 2〜48 文字にしてください。',
    wake_phrase_unpronounceable: (p) => `「${p.word}」の発音が見つかりません：一般的な英単語か、中国語のウェイクワードに変更してください。`,
    wake_engine_unsupported_os: (p) => `ウェイクワードと声紋は使用できません：macOS ${p.current} はウェイクエンジンに必要な ${p.required} より古いバージョンです。PA は起動時に互換コンポーネントを自動インストールします。この表示が続く場合は、インターネットに接続して PA を再起動するか、macOS ${p.required} 以降にアップデートしてください。`,
    wake_engine_unavailable: () => 'ウェイクエンジンを読み込めないため、ウェイクワードと声紋は使用できません：PA を再起動してください。解決しない場合はログを確認してください。',
    wake_model_missing: () => 'ウェイクモデルがダウンロードされていません：ウェイクワード設定でダウンロードしてください。',
    voiceprint_missing: () => '声紋が未登録です：先に声紋を録音するか、本人の声のみで起動する設定をオフにしてください。',
    voiceprint_enroll_failed: () => '声紋の録音に失敗しました：静かな場所で、5 秒間話して録音し直してください。',
    wake_mic_unavailable: () => 'マイクを使用できません：システム設定 → プライバシーとセキュリティ → マイク で Pocket Agent を許可してください。',
    wake_service_unavailable: () => '音声サービスの準備ができていません：数秒待ってから再度お試しください。続く場合は PA を再起動してください。',
    wake_service_error: () => 'ウェイクサービスでエラーが発生しました：PA を再起動してください。解決しない場合はログを確認してください。',
    saved: '設定を保存しました。',
    savedButWakeOff: (r) => `設定は保存しましたが、ウェイクワードは起動していません：${r}`,
  },
  ko: {
    wake_phrase_invalid: () => '웨이크 워드 형식이 올바르지 않습니다: 한자, 영문자, 공백으로 2–48자를 입력하세요.',
    wake_phrase_unpronounceable: (p) => `"${p.word}"의 발음을 찾을 수 없습니다: 흔한 영어 단어나 중국어 웨이크 워드로 바꾸세요.`,
    wake_engine_unsupported_os: (p) => `웨이크 워드와 성문을 사용할 수 없습니다: macOS ${p.current}은(는) 웨이크 엔진에 필요한 ${p.required}보다 오래된 버전입니다. PA는 시작할 때 호환 구성 요소를 자동으로 설치합니다. 이 안내가 계속 보이면 인터넷에 연결한 뒤 PA를 다시 시작하거나 macOS ${p.required} 이상으로 업데이트하세요.`,
    wake_engine_unavailable: () => '웨이크 엔진을 불러오지 못해 웨이크 워드와 성문을 사용할 수 없습니다: PA를 다시 시작하세요. 계속되면 로그를 확인하세요.',
    wake_model_missing: () => '웨이크 모델이 다운로드되지 않았습니다: 웨이크 워드 설정에서 모델을 다운로드하세요.',
    voiceprint_missing: () => '등록된 성문이 없습니다: 먼저 성문을 녹음하거나, 본인 목소리로만 깨우기 설정을 끄세요.',
    voiceprint_enroll_failed: () => '성문 녹음에 실패했습니다: 조용한 곳에서 5초 동안 말하며 다시 녹음하세요.',
    wake_mic_unavailable: () => '마이크를 열 수 없습니다: 시스템 설정 → 개인정보 보호 및 보안 → 마이크에서 Pocket Agent를 허용하세요.',
    wake_service_unavailable: () => '음성 서비스가 아직 준비되지 않았습니다: 몇 초 후 다시 시도하세요. 계속되면 PA를 다시 시작하세요.',
    wake_service_error: () => '웨이크 서비스에 오류가 발생했습니다: PA를 다시 시작하세요. 계속되면 로그를 확인하세요.',
    saved: '설정을 저장했습니다.',
    savedButWakeOff: (r) => `설정은 저장했지만 웨이크 워드가 시작되지 않았습니다: ${r}`,
  },
  fr: {
    wake_phrase_invalid: () => "Mot de réveil invalide : utilisez de 2 à 48 caractères chinois, lettres latines et espaces.",
    wake_phrase_unpronounceable: (p) => `Aucune prononciation trouvée pour « ${p.word} » : choisissez un mot anglais courant ou un mot de réveil en chinois.`,
    wake_engine_unsupported_os: (p) => `Mot de réveil et empreinte vocale indisponibles : macOS ${p.current} est antérieur à la version ${p.required} requise par le moteur de réveil. PA installe un composant compatible au démarrage ; si ce message persiste, connectez-vous à Internet et redémarrez PA, ou passez à macOS ${p.required} ou ultérieur.`,
    wake_engine_unavailable: () => "Le moteur de réveil n'a pas pu se charger : mot de réveil et empreinte vocale indisponibles. Redémarrez PA ; si le problème persiste, consultez le journal.",
    wake_model_missing: () => "Le modèle de réveil n'est pas téléchargé : téléchargez-le dans les réglages du mot de réveil.",
    voiceprint_missing: () => "Aucune empreinte vocale : enregistrez d'abord votre voix, ou désactivez le réveil par votre seule voix.",
    voiceprint_enroll_failed: () => "L'enregistrement de l'empreinte vocale a échoué : recommencez dans un endroit calme en parlant pendant 5 secondes.",
    wake_mic_unavailable: () => "Impossible d'ouvrir le micro : autorisez Pocket Agent dans Réglages Système → Confidentialité et sécurité → Micro.",
    wake_service_unavailable: () => "Le service vocal n'est pas encore prêt : patientez quelques secondes puis réessayez ; si cela continue, redémarrez PA.",
    wake_service_error: () => 'Le service de réveil a rencontré une erreur : redémarrez PA ; si le problème persiste, consultez le journal.',
    saved: 'Réglages enregistrés.',
    savedButWakeOff: (r) => `Réglages enregistrés, mais le mot de réveil n'a pas démarré : ${r}`,
  },
  de: {
    wake_phrase_invalid: () => 'Ungültiges Aktivierungswort: Verwende 2–48 chinesische Zeichen, lateinische Buchstaben und Leerzeichen.',
    wake_phrase_unpronounceable: (p) => `Keine Aussprache für „${p.word}“ gefunden: Verwende ein gängiges englisches Wort oder ein chinesisches Aktivierungswort.`,
    wake_engine_unsupported_os: (p) => `Aktivierungswort und Stimmabdruck sind nicht verfügbar: macOS ${p.current} ist älter als die für die Aktivierungs-Engine nötige Version ${p.required}. PA installiert beim Start eine kompatible Komponente; wenn diese Meldung bleibt, verbinde dich mit dem Internet und starte PA neu oder aktualisiere auf macOS ${p.required} oder neuer.`,
    wake_engine_unavailable: () => 'Die Aktivierungs-Engine konnte nicht geladen werden, Aktivierungswort und Stimmabdruck sind nicht verfügbar: Starte PA neu; wenn es weiter auftritt, sieh im Protokoll nach.',
    wake_model_missing: () => 'Das Aktivierungsmodell ist nicht heruntergeladen: Lade es in den Einstellungen zum Aktivierungswort herunter.',
    voiceprint_missing: () => 'Noch kein Stimmabdruck: Nimm zuerst deine Stimme auf oder schalte die Aktivierung nur durch deine Stimme aus.',
    voiceprint_enroll_failed: () => 'Aufnahme des Stimmabdrucks fehlgeschlagen: Nimm an einem ruhigen Ort erneut auf und sprich die vollen 5 Sekunden.',
    wake_mic_unavailable: () => 'Mikrofon kann nicht geöffnet werden: Erlaube Pocket Agent unter Systemeinstellungen → Datenschutz & Sicherheit → Mikrofon.',
    wake_service_unavailable: () => 'Der Sprachdienst ist noch nicht bereit: Warte einige Sekunden und versuche es erneut; wenn es so bleibt, starte PA neu.',
    wake_service_error: () => 'Im Aktivierungsdienst ist ein Fehler aufgetreten: Starte PA neu; wenn es weiter auftritt, sieh im Protokoll nach.',
    saved: 'Einstellungen gespeichert.',
    savedButWakeOff: (r) => `Einstellungen gespeichert, aber das Aktivierungswort wurde nicht gestartet: ${r}`,
  },
  es: {
    wake_phrase_invalid: () => 'Palabra de activación no válida: usa de 2 a 48 caracteres chinos, letras latinas y espacios.',
    wake_phrase_unpronounceable: (p) => `No se encontró la pronunciación de «${p.word}»: usa una palabra inglesa común o una palabra de activación en chino.`,
    wake_engine_unsupported_os: (p) => `La palabra de activación y la huella de voz no están disponibles: macOS ${p.current} es anterior a la versión ${p.required} que necesita el motor de activación. PA instala un componente compatible al iniciarse; si sigues viendo este aviso, conéctate a Internet y reinicia PA, o actualiza a macOS ${p.required} o posterior.`,
    wake_engine_unavailable: () => 'No se pudo cargar el motor de activación, así que la palabra de activación y la huella de voz no están disponibles: reinicia PA; si continúa, revisa el registro.',
    wake_model_missing: () => 'El modelo de activación no está descargado: descárgalo en los ajustes de la palabra de activación.',
    voiceprint_missing: () => 'Aún no hay huella de voz: graba tu voz primero o desactiva la activación solo con tu voz.',
    voiceprint_enroll_failed: () => 'No se pudo grabar la huella de voz: vuelve a grabar en un lugar tranquilo y habla durante los 5 segundos.',
    wake_mic_unavailable: () => 'No se puede abrir el micrófono: permite Pocket Agent en Ajustes del Sistema → Privacidad y seguridad → Micrófono.',
    wake_service_unavailable: () => 'El servicio de voz aún no está listo: espera unos segundos y vuelve a intentarlo; si sigue pasando, reinicia PA.',
    wake_service_error: () => 'El servicio de activación tuvo un error: reinicia PA; si continúa, revisa el registro.',
    saved: 'Ajustes guardados.',
    savedButWakeOff: (r) => `Ajustes guardados, pero la palabra de activación no se inició: ${r}`,
  },
};

function strings(lang: string): WakeErrorStrings {
  return WAKE_ERROR_TEXT[(SETTINGS_LANGS as readonly string[]).includes(lang) ? lang as SettingsLang : 'en'];
}

export function parseWakeError(e: unknown): WakeError | null {
  let v: unknown;
  try { v = JSON.parse(String(e)); } catch { return null; }
  if (!v || typeof v !== 'object' || typeof (v as { wake_error?: unknown }).wake_error !== 'string') return null;
  const { wake_error, params } = v as { wake_error: string; params?: unknown };
  const code = (WAKE_ERROR_CODES as readonly string[]).includes(wake_error) ? wake_error as WakeErrorCode : 'wake_service_error';
  const clean: Record<string, string> = {};
  if (params && typeof params === 'object') {
    for (const [k, val] of Object.entries(params)) clean[k] = String(val);
  }
  return { code, params: clean };
}

/** Localized problem + fix for any error from a wake or voiceprint command. */
export function wakeErrorMessage(lang: string, e: unknown): string {
  const err = parseWakeError(e);
  if (!err) console.warn('[wake] uncoded error:', e);
  const { code, params } = err ?? { code: 'wake_service_error' as const, params: {} };
  if (code === 'wake_phrase_empty') return convLabelsLang(lang).wakeNeedsPhrase;
  return strings(lang)[code](params);
}

/** Notice for a save that succeeded while the wake listener did not start. */
export function wakeSaveNotice(lang: string, e: unknown): string {
  const t = strings(lang);
  const reason = wakeErrorMessage(lang, e);
  return parseWakeError(e)?.code === 'wake_phrase_empty' ? `${t.saved} ${reason}` : t.savedButWakeOff(reason);
}
