# inbound

Локальная точка входа на **клиентской** стороне. Слушает локальный порт, делает хендшейк с приложением (браузер, curl и т.д.), достаёт адрес назначения.

## Ответственность

- Слушает локальный TCP-порт (например 1080 для SOCKS5)
- Принимает входящее соединение от приложения → `browser_socket`
- Делает хендшейк по протоколу приложения (SOCKS5, HTTP CONNECT)
- Извлекает `ConnectRequest { addr, port }`
- Отдаёт `(browser_socket, ConnectRequest)` дальше

## Что происходит дальше

После того как `inbound` сделал своё дело:
- `ConnectRequest` уходит в `protocol/` — тот зашивает addr+port в tunnel header
- `browser_socket` остаётся для relay loop
- `transport/` создаёт **отдельное** исходящее соединение до сервера
- Relay loop: `browser_socket ←→ upstream_socket`

## Структура

```
inbound/
  mod.rs      — Inbound трейт
  socks5/     — SOCKS5 v5 хендшейк
  http/       — HTTP CONNECT хендшейк
```

## Что модуль НЕ делает

- Не создаёт соединение до сервера (это `transport/`)
- Не знает про tunnel протокол, auth, TLS
- Не гоняет данные в relay — только хендшейк и отдаёт сокет
