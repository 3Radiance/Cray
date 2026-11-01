# Слой `xhttp2` (Split HTTP/2)

Слой `xhttp2` реализует разделение туннеля на два независимых HTTP/2-потока в рамках одного соединения (Split HTTP):
1. **Upload (клиент ──▶ сервер):** запрос с методом из конфигурации (обычно `POST`). Клиент пишет исходящие байты в тело запроса, а сервер их читает (`RecvStream`).
2. **Download (сервер ──▶ клиент):** запрос с методом `GET`. Сервер отправляет ответ и пишет байты в тело ответа (`SendStream`), а клиент читает их из ответа (`ResponseFuture`).

Оба потока связываются между собой на сервере с помощью уникального случайного 32-символьного hex Session ID (`sid`), передаваемого в кастомном HTTP-заголовке.

---

## Кастомный заголовок Session ID (`id`)

Слой `xhttp2` **не навязывает фиксированное имя заголовка** для Session ID. Имя заголовка полностью определяется через профиль HTTP/2 (`config`), где в таблице `[http_headers]` для нужного заголовка указывается специальное значение `"id"`:

```toml
# conf/http2.toml
[http_headers]
"x-session-id" = "id"          # Сюда Cray подставит сгенерированный Session ID
"user-agent"   = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"
"accept"       = "*/*"
```

### Как это работает:
- **Значение `"id"`** — это маркер для Cray.
- **На клиенте (`outbound`):** перед отправкой запросов `POST` и `GET` слой генерирует случайный 32-значный hex идентификатор (`sid`) и подставляет его значением в указанный заголовок.
- **На сервере (`inbound`):** сервер находит в профиле заголовок, у которого значение `"id"`, и извлекает Session ID из входящих запросов клиента для корреляции пары `POST` и `GET`.
- **Имя заголовка может быть любым допустимым HTTP-заголовком**, например:
  - `"x-session-id" = "id"`
  - `"x-correlation-id" = "id"`
  - `"sec-ch-ua-session" = "id"`
- **⚠️ Требование:** и клиенту, и серверу в своём файле профиля **обязательно** нужно иметь заголовок со значением `"id"` как для upload-ноги (метод слоя), так и для download-ноги (`GET`). Заголовок ищется в секции соответствующей ноги (`[uplink]`/`[downlink]`), а при её отсутствии — в общем наборе `headers_order`/`[http_headers]`. Если в какой-то ноге заголовок со значением `"id"` не найден, конфигурация слоя не соберётся с ошибкой `XHTTP2 needs a header with the value id (.../uplink|downlink)`.

---

## Конфигурация слоя

Слой `xhttp2` настраивается одинаково просто как на клиенте, так и на сервере:

```toml
# Клиент (outbound)
[[outbound.layers]]
type = "xhttp2"
host = "example.com"
path = "/split-tun"
method = "POST"
config = "conf/http2.toml"

# Сервер (inbound)
[[inbound.layers]]
type = "xhttp2"
host = "example.com"
path = "/split-tun"
method = "POST"
config = "conf/http2.toml"
```

### Параметры

| Поле | Тип | Описание |
|---|---|---|
| `host` | строка | Обязательно. Хост / `:authority` запроса. На сервере запросы с другим хостом отклоняются с кодом `reject_status` (по умолчанию 404). |
| `path` | строка | Обязательно. Путь (`:path`), на который отправляются и ожидаются запросы. Все остальные пути получают `reject_status`. |
| `method` | строка | Обязательно. Метод для upload-ноги (`POST`, `PUT` и т.д.). Download-нога всегда использует `GET`. |
| `config` | строка | Обязательно. Путь к файлу профиля HTTP/2 (нужен и outbound, и inbound, чтобы знать заголовок с `id`). |

---

## Профиль: uplink и downlink, ответы и отказы

Заголовки запросов можно задавать отдельно для upload-ноги (`[uplink]` — любой метод, кроме `GET`) и отдельно для download-ноги (`[downlink]` — всегда `GET`). Старый плоский формат продолжает работать: верхнеуровневые `headers_order` и `[http_headers]` служат общим фолбэком для обеих ног. Внутри каждой ноги — те же `headers_order` и `http_headers`, при частичном задании недостающее берётся из верхнего уровня (у `[uplink]` есть алиасы `[post]`/`[put]`, у `[downlink]` — `[get]`):

```toml
[uplink]
headers_order = ["x-session-id", "user-agent", "content-type"]

[uplink.http_headers]
"x-session-id" = "id"
"user-agent" = "Mozilla/5.0"
"content-type" = "application/grpc"

[downlink]
headers_order = ["x-session-id", "user-agent"]

[downlink.http_headers]
"x-session-id" = "id"
"user-agent" = "Mozilla/5.0"
```

### Ответы сервера

- `[uplink.response]` / `[downlink.response]` — заголовки успешных (200) ответов на соответствующий запрос. Формат тот же: `headers_order` + `http_headers`.
- Если `response` у ноги не задан, используется верхнеуровневая таблица `[response]` (общий фолбэк); если нет и её, ответ содержит только `content-type: application/octet-stream`.
- В заголовках ответа (как и в запросных) значение `"id"` подменяется на Session ID.
- Если в ответных заголовках не указан `content-type`, слой ставит `application/octet-stream`.

### Отказы

Запросы, не совпавшие по пути, хосту, методу или с отсутствующим/битым Session ID, отклоняются:

- `reject_status` — код отказа, любое значение 100–999 (по умолчанию `404`); некорректный код ломает загрузку профиля.
- `[reject]` — заголовки отказа в том же формате; при отсутствии таблицы отказ остаётся голым (только статус).
- `body_path` внутри `[reject]` — путь к файлу (обычно HTML), целиком отдающемуся телом отказа. Файл читается один раз при загрузке профиля, отсутствие файла — ошибка конфигурации. Если `content-type` в `[reject.http_headers]` не задан, при наличии тела подставляется `text/html`.

```toml
reject_status = 505

[response]
headers_order = ["server"]

[response.http_headers]
server = "nginx"

[uplink.response]
headers_order = ["content-type", "data"]

[uplink.response.http_headers]
content-type = "application/grpc"
data = "id"               # подменяется на Session ID

[reject]
headers_order = ["server", "content-type"]
body_path = "conf/reject.html"

[reject.http_headers]
server = "nginx"
content-type = "text/html; charset=utf-8"
```

`reject_status`, `[reject]` и `[response]` работают и в слое `http2` (без uplink/downlink — одним общим набором, см. [http2.md](http2.md)).

---

## Архитектура и стриминг

```
Клиент (Outbound)                                        Сервер (Inbound)
─────────────────                                        ────────────────
[ChunkStream]                                            [ChunkStream]
    │                                                        │
    ├─▶ TX: POST /split-tun (тело запроса) ───▶ RX (чтение) ─┤
    │       Header: x-session-id: <sid>                      │
    │                                                        │
    └─◀ RX: GET  /split-tun (тело ответа)  ◀─── TX (запись) ─┘
            Header: x-session-id: <sid>
```

- Порядок прихода запросов на сервер не имеет значения: если первым пришел `POST`, его поток чтения сохраняется до прихода `GET`. Если первым пришел `GET`, его дескриптор ответа ждет прихода `POST`.
- Данные стримятся чанками (`ChunkStream`) без полной буферизации в памяти.
- Окно управления потоком HTTP/2 автоматически обновляется при чтении данных.
