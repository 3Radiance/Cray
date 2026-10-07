<p align="left">
  <img src="./cray.jpg" width="15%" alt="crays-logo" style="border-radius: 16px; box-shadow: 0 4px 20px rgba(0,0,0,0.15);">
</p>

# Cray

Кастомный прокси-туннель на Rust. Написан под себя — никаких компромиссов ради совместимости с чужими клиентами.

## Ключевые идеи

### Конфигурируемые протоколы
Протокол туннеля не захардкожен. Есть два способа задать свой:

1. **YAML DSL** — описать структуру полей декларативно без Rust-кода. Поля, типы, верификаторы из закрытого реестра, реакция на несовпадение. Для тех кто не хочет писать код.
2. **Rust-код в `protocol/`** — реализовать `Protocol` трейт напрямую. Полная свобода, полная ответственность.

Один YAML описывает протокол для обеих сторон — клиент и сервер читают один файл.

### Закрытый реестр верификаторов
`verify: hmac_check` в DSL — это имя из фиксированного набора (`hmac_check`, `timestamp_fresh`, `range_check`, ...). Пользователь выбирает функцию по имени, но не может переопределить её логику. Весь крипто-код живёт в `auth/` и только там.

### Конвейерный транспорт
Транспортные слои собираются в цепочку по конфигу:
```
[tcp | udp] → [tls | кастомный] → [http | http2 | http3 |кастомный]
```
Добавить свой транспортный слой = написать трейт + указать в конфиге.

### Двухуровневая валидация
- **Уровень 1 (nginx)**: HMAC-кука, timestamp, nonce-кэш. Нет куки → честный статик-сайт.
- **Уровень 2 (бэкенд)**: бинарный auth_token внутри тела/протокола. Не совпал → зеркало реальной страницы ошибки. Никогда не TCP RST.

---

## Архитектура

```
[браузер / приложение]
        ↓
   inbound/          SOCKS5 / HTTP CONNECT → ConnectRequest
        ↓
   transport/        tcp → tls → http | http2 | кастомный
        ↓
   protocol/         application-level хендшейк, auth → ConnectRequest
        ↓
   outbound/         TCP-коннект до назначения
        ↓
[сервер назначения]
```

**Relay loop** после всех хендшейков:
```
browser_socket  ←→  tunnel_stream  ←→  remote_socket
```

---

## Модули

| Модуль | Роль |
|---|---|
| [`inbound/`](src/inbound/README.md) | Локальный вход: SOCKS5, HTTP CONNECT |
| [`transport/`](src/transport/README.md) | Конвейер: tcp → tls → http/http2 |
| [`protocol/`](src/protocol/readme.md) | Application-level хендшейк и auth |
| [`outbound/`](src/outbound/README.md) | Коннект до назначения |
| [`auth/`](src/auth/README.md) | Закрытый реестр верификаторов/генераторов |
| [`config/`](src/config/README.md) | Оркестратор, DSL-валидатор |
| [`fingerprint/`](src/fingerprint/readme.md) | TLS/HTTP2/TCP профили |

---

## Зависимости между модулями

```
config ──► auth
       ──► fingerprint
       ──► protocol
       ──► transport

transport ──► fingerprint
          ──► protocol (вызывается внутри конвейера)

protocol ──► auth (верификаторы)

outbound   ← не зависит ни от кого кроме ConnectRequest
auth       ← не зависит ни от кого
fingerprint ← не зависит ни от кого
```

Стрелки только вниз, циклов нет.

---

## Статус

🚧 В разработке. Архитектура заложена, реализация начата.

**Порядок итераций:**
1. `auth/` — верификаторы и генераторы изолированно
2. `fingerprint/` — TLS/HTTP2 профили (переиспользуется из MITM-проекта)
3. Фиксированный `protocol/tsf/` — первый рабочий туннель
4. `transport/` конвейер + relay loop
5. YAML DSL поверх работающей системы
