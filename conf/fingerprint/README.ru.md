# wreq-util emulation profiles → cray fingerprint TOML

[Read in english](README.md)

Набор готовых TLS/HTTP2-профилей для слоёв `tls` и `http2`.

Это **не копия wreq**. Значения (шифры, кривые, порядок расширений, SETTINGS,
заголовки) перенесены из [wreq-util](https://github.com/0x676e67/wreq-util)
(`src/emulate/profile`) — там они свёрстаны по живым браузерам — но дальше
это самостоятельные TOML-профили в собственном формате cray
(`layers/src/profiles/{tls.rs,http2.rs}` → TlsConfig / Http2Config).
Формат настроек cray сознательно кастомный и гибкий: любое поле профиля
можно менять руками, не оглядываясь на wreq. Сами профили нужны для
удобства — это пресеты-стартовые точки, а не жёсткая зависимость: если нужен
свой отпечаток, возьмите ближайший файл и правьте значения как хотите.

> **⚠ Файлы сгенерированы.** Каждый `.toml` здесь создан скриптом
> `tools/gen.py` из исходников wreq-util: их не писали руками и не снимали с
> реальных браузеров. Конвертер неидеален, поэтому в отдельных значениях
> могут быть ошибки и опечатки. Тест ниже лишь доказывает, что профиль
> принимается `Engine::build`, а не то, что отпечаток совпадает с настоящим
> браузером. Перед использованием сверяйте профиль с реальным дампом, а
> системные проблемы исправляйте в `tools/gen.py`.

## Структура

    conf/fingerprint/<browser>/<profile>/
        tls.toml     → TLS-часть (TlsConfig)
        http2.toml   → HTTP/2-часть (Http2Config), включая headers_order/[http_headers]

Имена папок = идентификаторы модов в wreq-util
(`chrome.rs`, `firefox.rs`, `safari.rs`, `opera.rs`, `okhttp.rs`).
Алиас-моды (напр. `v133 → v132::build_emulation`) наследуют TLS и SETTINGS
от профиля-рута, но имеют свои UA/заголовки. Edge лежит в `chrome/`
(рядом с root-версиями), Opera — отдельная папка.

## Кого сколько

| папка    | профилей | содержание                                          |
|----------|----------|-----------------------------------------------------|
| chrome/  | 65       | v100..v154 + edge101..edge148                        |
| firefox/ | 21       | ff109..ff152, ff_android_135, ff_private_*           |
| safari/  | 29       | safari15_3..safari26_4, safari_ios_*, safari_ipad_*  |
| opera/   | 16       | opera116..opera131                                   |
| okhttp/  | 8        | okhttp3_9..okhttp5                                   |

## Как валидировать

    cargo test -p cray-layers --test profiles
    # тест generated_wreq_emulation_profiles_are_accepted гоняет все папки через полный Engine::build

## Примечания к конверсии

Поля, которые нельзя выразить схемой cray, помечены `# NOTE:` прямо в
файлах. Их немного:

- `TLS_AES_128_CCM_SHA256` / `TLS_AES_256_CCM_8_SHA256` (okhttp3_13) удалены:
  btls BoringSSL их не поддерживает.
- `mldsa44/65/87` (chrome v150+) удалены: в btls(-sys) 0.5.6 BoringSSL нет
  ML-DSA sigalgs (это API fork-ревизии btls, которую использует wreq).
- `pre_shared_key` (PSK) не выразим: нужен client session cache — его пока
  нет ни в btls-0.5.6, ни в cray.
- `grease_sigalgs_enabled` / `random_aes_hw_override` (chrome 152/154, firefox)
  — API fork-btls, в обычном btls их нет.
- `connection_window_update` = `initial_connection_window_size - 65535`
  (поле у cray — размер WINDOW_UPDATE, а не полное окно).
- `enable_push` — обязательный bool в `[settings]`; если wreq его не задаёт
  (не шлёт в SETTINGS), он всё равно прописан, но отсутствует в
  `settings_order`, поэтому в фрейм не попадает.
- `enable_connect_protocol` / `no_rfc7540_priorities` у всех семейств стоят в
  `settings_order` без значений (как у wreq — id без значения не
  отправляется); реальное значение ставит только Safari 18+ (оба `true`).

Всё остальное из старого списка «не хватает» уже поддерживается схемой:
`key_shares`, `min_tls_version`/`max_tls_version`, `alps_use_new_codepoint`,
`trust_anchors` (sorted/shuffled), `aes_hw_override`, начальный
`initial_stream_id`, `priority_frames`, ALPS.

## Генерация

Профили генерируются из исходников wreq-util:

    tools/fetch_wreq_profiles.sh   # копирует src/emulate/profile в tools/profile/
    python3 tools/gen.py           # парсит Rust-исходники, пишет conf/fingerprint/**

`fetch_wreq_profiles.sh` берёт wreq-util на зафиксированной ревизии
(`917d2581d6761a94db66466caa6c2eb4d4cdbd43`, из неё сгенерированы лежащие в
репозитории файлы); чтобы попробовать другую, передайте коммит или тег первым
аргументом. Папка `tools/profile/` в git-ignore и в репозиторий не входит.
Править файлы руками можно и нужно — но при регенерации ручные правки
сотрутся, так что постоянные отличия лучше вносить в `tools/gen.py`.

## Лицензия и авторство

Значения профилей взяты из [wreq-util](https://github.com/0x676e67/wreq-util),
© 0x676e67. На зафиксированной ревизии он выпущен под Apache License 2.0;
более старые релизы (например 3.0.0-rc.x) были под LGPL-3.0, поэтому
проверяйте лицензию той ревизии, из которой генерируете. Сам Cray под
GPL-3.0, с которой код под Apache-2.0 совместим. Если распространяете эти
файлы, сохраняйте это указание авторства.
