# fingerprint/http2

Создаёт HTTP/2 builder с нужными настройками из конфига. Чисто конфигурация — никаких соединений.

## Ответственность

- Принимает HTTP/2 профиль из `config`
- Собирает builder с параметрами SETTINGS фрейма:
  - `INITIAL_WINDOW_SIZE`
  - `HEADER_TABLE_SIZE`
  - `MAX_CONCURRENT_STREAMS`
  - `MAX_FRAME_SIZE`
- Настраивает header order и псевдохедеры (для fingerprint под конкретный браузер/клиент)

## Что отдаёт

- Готовый builder → идёт в `transport/http2/` для настройки соединения

## Что модуль НЕ делает

- Не устанавливает соединения сам
- Не делает хендшейк — только собирает конфигурацию
- Не знает про TLS (это `fingerprint/tls/`)