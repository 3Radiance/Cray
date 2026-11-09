# Написание слоёв

Слои — строительные блоки Cray. Каждый слой делает одно дело: принимает
соединение, трансформирует поток или реализует протокол. Слои компонуемы —
движок собирает их в цепочку по вашей конфигурации и валидирует её при старте
программы.

На этой странице: контракт слоя, полный разбор маленького слоя, Ports, `Cx`,
таймауты, логирование и тестирование. Низкоуровневые детали обёрток на
`AsyncRead`/`AsyncWrite` — в [stream-wrappers.md](stream-wrappers.md).

[Read in English ->](../eng/writing-layers.md)

## Контракт слоя

Каждый слой реализует трейт `Layer` из `core/src/api.rs`:

| Метод | Назначение | По умолчанию |
|-------|-----------|--------------|
| `ports()` | Что слой принимает и отдаёт; по нему движок проверяет порядок цепочки | обязательный |
| `requires()` | Ключи `Cx`, которые слою нужны от предыдущих слоёв | пусто |
| `provides()` | Ключи `Cx`, которые слой кладёт для других слоёв | пусто |
| `default_timeout()` | Таймаут для `call()`, если конфиг молчит | `None` |
| `call()` | Основная логика: принять `Input`, вернуть `Output` | обязательный |

`Input` — это либо `Input::None` (пока ничего — так получает слой-источник),
либо `Input::Stream(BoxedStream)`. `Output` — один из вариантов: `Stream`
(один стрим следующему слою), `Streams` (акцептор соединений — каждое
принятое соединение запускает новый проход оставшейся цепочки), `Done`
(терминальный слой закончил) или `Consumed`.

С точки зрения движка сессии без состояния: слой, которому нужно общее
состояние (таблицы сессий, счётчики), владеет им внутри себя.

## Анатомия слоя

Каждый слой состоит из **трёх частей**:

| Часть | Назначение |
|-------|------------|
| **Структура конфигурации** | Десериализует параметры TOML из файла конфигурации |
| **Структура слоя** | Хранит состояние выполнения; реализует трейт `Layer` |
| **Функция `build()`** | Фабрика, которая парсит конфигурацию и создаёт слой |

Настоящие слои живут в `layers/src`, сгруппированы по типу:
`transport/`, `proxy/`, `protocol/`, `wrappers/`, плюс `tcp.rs`, `unix.rs`.

## Пошаговый разбор: слой `fragment`

Разберём настоящий небольшой слой, `fragment` из
[`layers/src/wrappers/fragment.rs`](../../layers/src/wrappers/fragment.rs).
Он принимает поток и режет каждую запись на более мелкие части.

### 1. Определите структуру конфигурации

```rust
use serde::Deserialize;

use crate::deserialize_from_str;

#[derive(Debug, Deserialize, Clone)]
pub struct FragStage {
    pub bytes: Option<u64>,          // верхняя граница стадии в суммарно записанных байтах
    #[serde(deserialize_with = "deserialize_from_str")]
    pub size: FragSize,              // парсится из TOML-строки вида "600-1000"
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentCfg {
    stages: Option<Vec<FragStage>>,
}
```

Структура конфигурации напрямую соответствует TOML-таблице слоя:

```toml
[[outbound.layers]]
type = "fragment"
stages = [
    { bytes = 2000, size = "1-1" },
    { size = "600-1000" },
]
```

Замечания:

- `#[serde(deny_unknown_fields)]` отвергает неизвестные поля — опечатка
  падает с внятной ошибкой, а не игнорируется тихо.
- `deserialize_from_str` — хелпер в `layers/src/lib.rs`: прогоняет строковое
  значение (здесь `"1-1"`) через `FromStr` типа и отдавает ошибку как ошибку
  конфигурации.
- `params.clone().try_into()?` в `build()` — так фабрика превращает
  TOML-таблицу в структуру: благодаря `deny_unknown_fields` таблица обязана
  совпасть строго.

### 2. Определите структуру слоя

```rust
pub struct Fragment {
    stages: Vec<FragStage>,
}
```

Поля `Side` здесь нет — `fragment` работает одинаково inbound и outbound.
Когда оно нужно, смотрите [Поведение по сторонам](#поведение-по-сторонам)
ниже.

### 3. Реализуйте трейт `Layer`

```rust
use anyhow::Result;
use cray_core::{BoxFuture, BoxedStream, Cx, Input, Layer, Output, Ports};

impl Layer for Fragment {
    fn ports(&self) -> Ports {
        Ports::WRAP // принимает один поток, отдаёт один
    }

    fn call<'a>(&'a self, input: Input, _cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let stream: BoxedStream = input.stream()?;
            Ok(Output::Stream(Box::pin(FragmentStream::new(
                stream,
                self.stages.clone(),
            ))))
        })
    }
}
```

`input.stream()?` распаковывает `Input::Stream` и падает с ясной ошибкой,
если цепочка дала слою не поток. У `requires()`, `provides()` и
`default_timeout()` есть реализации по умолчанию, поэтому слой, которому
ничего не нужно, может их не писать.

### 4. Напишите функцию `build()`

```rust
use anyhow::{Result, bail};
use cray_core::{BuildCx, Layer};

pub fn build(params: &toml::Table, _bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: FragmentCfg = params.clone().try_into()?;
    let stages = cfg
        .stages
        .ok_or_else(|| anyhow::anyhow!("stages is required"))?;
    if stages.is_empty() {
        bail!("stages must not be empty");
    }
    Ok(Box::new(Fragment { stages }))
}
```

Всё, что можно отвергнуть без запуска — неизвестные поля, отсутствие
обязательных, невалидные размеры, — должно жить здесь, а не в `call()`.
Движок собирает цепочки на старте и падает сразу, поэтому плохая конфигурация
до `call()` не доходит.

### 5. Зарегистрируйте слой

Имена слоёв — ключи в `Registry`, который наполняется в
[`layers/src/lib.rs`](../../layers/src/lib.rs):

```rust
pub fn register(r: &mut Registry) {
    // ...
    r.add("xhttp2", transport::xhttp2::build);
    r.add("pipe", protocol::pipe::build);
}
```

Создайте файл модуля (например, `layers/src/wrappers/fragment.rs`) и
добавьте `pub mod fragment;` в родительский `mod.rs`. Бинарник в
`cray/src/main.rs` вызывает `cray_layers::register()`, так что больше ничего
не нужно. Повторная регистрация имени паникует на старте.

Имя в `r.add("...")` — это значение `type = "..."` в TOML. Имена слоёв —
строчные, через дефис.

## Понимание `Ports`

`ports()` сообщает движку, что делает ваш слой. Значение `Ports` — пара
`(takes, gives)` из `Port::{Nothing, Stream, Streams, Done}`. Выберите
правильную константу:

| Константа | Принимает | Отдаёт | Случай использования |
|-----------|-----------|--------|----------------------|
| `Ports::SOURCE` | `Nothing` | `Stream` | Создаёт одно соединение: исходящий дозвон по сокету или входящий источник, отдающий один стрим вместо приёма множества |
| `Ports::LISTENER` | `Nothing` | `Streams` | Постоянно принимает входящие соединения (inbound-прослушивание) |
| `Ports::WRAP` | `Stream` | `Stream` | Превращает один поток в другой |
| `Ports::ACCEPT` | `Stream` | `Streams` | Мультиплексирование: принимает запросы на потоке и отдаёт каждый отдельным стримом |
| `Ports::TERMINAL` | `Stream` | `Done` | Обработчик протокола — потребляет поток |

Настоящие слои:

- `tcp` / `unix`: `SOURCE` outbound, `LISTENER` inbound;
- `tls`: `WRAP` — принимает поток, отдаёт (рас)шифрованный, в обе стороны;
- `http-proxy`: `WRAP` — CONNECT сквозь предыдущий стрим, в обе стороны;
- `http2` / `xhttp2`: `WRAP` outbound (один запрос → один виртуальный стрим)
  и `ACCEPT` inbound (принимает запросы, каждый подходящий отдаёт отдельным
  виртуальным стримом);
- `vless` и `pipe`: `TERMINAL`.

Правила цепочки, проверяемые движком на старте:

- первый слой обязан принимать `Nothing`;
- соседние слои обязаны состыковываться (`Stream`/`Streams` → `Stream`);
- последний входящий слой всё ещё должен отдавать поток — всё, что даёт
  `Done` посреди списка, это ошибка;
- слои протоколов (`gives = Done`) обязаны стоять непрерывно в конце списка,
  и ровно один из них — терминальный;
- каждый слой outbound-цепочки обязан отдавать
  ровно один поток, чтобы им можно было пользоваться для дозвона.

## Использование `requires()`, `provides()` и `Cx`

Слои общаются через общий объект `Cx` (контекст). Это отображение ключей
типов на значения: `key::<T>()` возвращает `Key` для типа `T`, `cx.put(v)`
сохраняет значение по его типу, `cx.get::<T>()` читает его обратно. Значения
обязаны быть `Send + Sync`; фиксированной схемы нет — подойдёт любой тип, а
движок на старте проверяет, что каждый `requires()` удовлетворяется каким-то
более ранним `provides()` в цепочке.

**Пример**: слой рукопожатия создаёт ключи сессии, слой шифрования их
потребляет:

```rust
// Определите вашу общую структуру
pub struct SessionKeys {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
}

use anyhow::{anyhow, Result};
use cray_core::{BoxFuture, Cx, Input, Output};

// Слой-производитель — сохраняет ключи в Cx
impl Layer for Handshake {
    fn provides(&self) -> Vec<cray_core::Key> {
        vec![cray_core::key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let mut stream = input.stream()?;
            let mut keys = SessionKeys { send_key: [0; 32], recv_key: [0; 32] };
            do_handshake(&mut stream, &mut keys).await?;
            cx.put(keys);  // теперь доступно последующим слоям
            Ok(Output::Stream(stream))
        })
    }
}

// Слой-потребитель — читает ключи из Cx
impl Layer for Crypto {
    fn requires(&self) -> Vec<cray_core::Key> {
        vec![cray_core::key::<SessionKeys>()]
    }

    fn call<'a>(&'a self, input: Input, cx: &'a mut Cx) -> BoxFuture<'a, Result<Output>> {
        Box::pin(async move {
            let keys = cx
                .get::<SessionKeys>()
                .ok_or_else(|| anyhow!("нет SessionKeys в Cx"))?;
            let stream = input.stream()?;
            Ok(Output::Stream(stream)) // ... здесь обёртка использует ключи ...
        })
    }
}
```

(`do_handshake` здесь — сокращение для вашего собственного кода рукопожатия.)

### Встроенные ключи контекста

Два встроенных типа уже живут в контексте, оба описаны в `core/src/api.rs` и
`core/src/dial.rs`:

- **`ConnectRequest`** — адрес назначения для этого соединения
  (`addr: String`, `port: u16`). Реальное значение в контекст кладут слои,
  стоящие раньше по цепочке: входящий `http-proxy` разбирает HTTP CONNECT и
  кладёт туда разобранную цель. На старте движок уже знает, откуда возьмётся
  значение для outbound-слоёв: при дозвоне dialer копирует контекст и
  вставляет цель (`Dialer::dial_to`), поэтому outbound-слои и стадии дозвона
  могут объявлять `ConnectRequest` в `requires()`. `tcp` outbound без `addr`
  дозванивается до него. Терминальный слой может требовать его только если
  что-то раньше по его цепочке это предоставляет.
- **`Dialer`** — как создать исходящее соединение. Движок сам подкладывает
  `Dialer` терминальному слою и промежуточным стадиям; объявлять его в
  `provides()` не нужно. `Dialer::dial_to(cx, target)` дозванивается до цели
  по вложенной outbound-цепочке и кладёт её `ConnectRequest` в копию
  контекста стадии.

## Поведение по сторонам

`build()` получает `BuildCx { side: Side }` — `Side::Inbound` или
`Side::Outbound`. Хранить `side` в структуре слоя вообще говоря не нужно
(`pipe`, например, его не хранит); храните, если слой ведёт себя по-разному
на inbound и outbound.

Слои, хранящие его, ветвятся в `ports()`, `requires()` и `call()`:

```rust
fn ports(&self) -> Ports {
    match self.side {
        Side::Inbound => Ports::LISTENER,
        Side::Outbound => Ports::SOURCE,
    }
}

fn requires(&self) -> Vec<Key> {
    match self.side {
        Side::Outbound if self.addr.is_none() => vec![key::<ConnectRequest>()],
        _ => Vec::new(),
    }
}
```

(из `layers/src/tcp.rs` — настоящий код)

Проверяйте конфигурацию с учётом стороны в `build()`:

```rust
pub fn build(params: &toml::Table, bcx: &BuildCx) -> Result<Box<dyn Layer>> {
    let cfg: HTTPConnectCfg = params.clone().try_into()?;
    if cfg.addr.is_none() && bcx.side == Side::Outbound {
        return Err(anyhow!("Client HTTPConnect Need addr"));
    }
    // ...
}
```

(из `layers/src/proxy/http.rs` — настоящий код)

## Таймаут по умолчанию и таймауты в конфиге

Движок оборачивает вызовы `call()` слоёв в таймаут в inbound- и
outbound-цепочках и в дозвоне (`core/src/engine.rs`, `core/src/dial.rs`):

- слой объявляет дефолт через `default_timeout()`:

  ```rust
  fn default_timeout(&self) -> Option<Duration> {
      Some(Duration::from_secs(5))
  }
  ```
- `timeout = <секунды>` в TOML на конкретном слое переопределяет его;
- именованный пресет из таблицы `[timeouts]` на верхнем уровне переопределяет
  тоже (`timeout = "handshake"` → `timeouts.handshake` в секундах);
- `timeout = 0` выключает таймаут для этого слоя.

Дефолт работает только там, где конфиг молчит. По таймауту `call()`
отменяется, а соединение рвётся с ошибкой `timed out after …`.

## Логирование

Пользуйтесь тегированным логированием для согласованного вывода:

```rust
use cray_core::log_tag;

log_tag!(debug, "HTTP", "CONNECT {}:{}", cr.addr, cr.port);
log_tag!(trace, "JITTER", "write {n}");
log_tag!(warn, "INB", "accept failed: {}", e);
```

Тег попадает и в `tracing`-таргет, и в префикс `[TAG]` к тексту сообщения.
Используйте имя вашего слоя в верхнем регистре без разделителей (`FRAG` для
`fragment`). Тег можно матчить в `RUST_LOG`:

```bash
RUST_LOG=info,TLS=debug,FRAG=trace ./cray --config client.toml
```

Теги, реально используемые в коде:

| Тег | Кто пишет под ним |
|-----|-------------------|
| `CHAIN` | `core` — движок, сборка цепочек, dialer |
| `INB` | входящие слушатели (`tcp`, `unix`) |
| `HTTP` | `http-proxy` |
| `TLS` / `CERT` | `tls`, сборка TLS и сертификаты |
| `H2` | `http2` |
| `XH2` | `xhttp2` |
| `VLESS` | `vless` |
| `JITTER` / `FRAG` / `PAD` | `jitter`, `fragment`, `pad` |

## Как протестировать слой

Интеграционные тесты живут в `layers/tests/` (`pad.rs`, `auth.rs`,
`http2.rs`, `tls.rs`, `xhttp2.rs`, `reorder.rs`, `gc.rs`, `profiles.rs`).
Канонический паттерн для обёрток — [`layers/tests/pad.rs`](../../layers/tests/pad.rs):

1. **Соберите слой так же, как это делает движок** — через настоящий
   реестр, заодно проверяя регистрацию:

   ```rust
   use cray_core::*;
   use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

   fn try_pad(stages: &str, side: Side) -> anyhow::Result<Box<dyn Layer>> {
       let mut reg = Registry::new();
       cray_layers::register(&mut reg);
       let f = reg.get("pad").expect("pad registered");
       let params: toml::Table = format!("stages = [{stages}]").parse().unwrap();
       f(&params, &BuildCx { side })
   }
   ```

2. **Прогоните слой через пару duplex** — вызовите слой напрямую, заверните
   оба конца и проверьте roundtrip:

   ```rust
   async fn roundtrip(stages: &str, msg: &[u8]) -> Vec<u8> {
       let (a, b) = duplex(65536);
       let mut up = wrap(&*pad_layer(stages, Side::Outbound), Box::pin(a) as BoxedStream).await;
       let mut dn = wrap(&*pad_layer(stages, Side::Inbound), Box::pin(b) as BoxedStream).await;

       up.write_all(msg).await.unwrap();
       up.shutdown().await.unwrap();

       let mut out = Vec::new();
       dn.read_to_end(&mut out).await.unwrap();
       out
   }
   ```

3. **Проверьте формат на проводе напрямую** — где слой добавляет фрейминг,
   заберите сырую сторону duplex и разберите её байты, как в `pad.rs`
   (`frames(&wire)`, `wire_of(...)`).
4. **Проверьте валидацию при сборке** — вызовите фабрику с плохими
   параметрами и сверьте текст ошибки, как `build_err(...)` в `pad.rs`.
5. **Проверьте частичные записи** — маленький буфер duplex (100 байт)
   делает каждую запись кадра частичной; именно так `pad.rs` гоняет свой
   цикл выгрузки (`survives_a_tiny_pipe`).

Прогон всего набора: `cargo test --workspace`, одного крейта:
`cargo test -p cray-layers`.

## Чек-лист нового слоя

- код: один модуль в `layers/src/`, регистрация в `layers/src/lib.rs`
- тесты в `layers/tests/`
- страница в `docs/eng/` **и** `docs/ru/`
- обновлённые `README.md` / `README.ru.md`, если слой меняет список
  возможностей

Процессные требования (ветки, формат коммитов) — в
[CONTRIBUTING.md](../../CONTRIBUTING.md).
