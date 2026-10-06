# fingerprint

Модуль TLS/HTTP2/TCP профилей. Принимает настройки из `config` и собирает готовые объекты конфигурации для транспортных слоёв.

## Ответственность

- Сборка TLS acceptor/connector по профилю (версии, cipher suites, ALPN, session tickets и т.д.)
- HTTP/2 параметры (SETTINGS frame: window size, header table size, max concurrent streams)
- TCP параметры (nodelay, keepalive, recv/send buffer)
- Отдаёт готовые объекты — `tls/` и `http2/` берут их без самостоятельной конфигурации

## Зависимости

- Получает настройки от `config`
- Используется `transport/tls/` и `transport/http2/`
- Не зависит от `auth`, `protocol`

## Что модуль НЕ делает

- Не устанавливает соединения сам
- Не знает про протоколы и auth
- Не держит состояние соединений