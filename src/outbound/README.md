# outbound

Финальная точка на **серверной** стороне. Получает `ConnectRequest` от `protocol/` и делает реальный TCP-коннект до адреса назначения.

## Ответственность

- Принимает `ConnectRequest { addr, port }` от `protocol/`
- Создаёт исходящее TCP-соединение до назначения → `remote_socket`
- Отдаёт `remote_socket` для relay loop

## Что происходит дальше

После того как `outbound` поднял соединение:
- `tunnel_socket` — сокет от клиента (из `transport/`)
- `remote_socket` — сокет до назначения (из `outbound/`)
- Relay loop: `tunnel_socket ←→ remote_socket`

## Структура

```
outbound/
  mod.rs      — Outbound трейт
  direct/     — прямой TCP-коннект до назначения
```

## Что модуль НЕ делает

- Не знает про tunnel протокол, auth, TLS между клиентом и сервером
- Не парсит пакеты — `ConnectRequest` уже готов к этому моменту
- Не гоняет relay loop — только создаёт соединение и отдаёт сокет
