# Tidyfile

[English](README.md) · **Español**

Un organizador de archivos gratuito, de código abierto, multiplataforma y con interfaz gráfica.

Define reglas visuales —*"si es un PDF y el nombre contiene 'factura', muévelo a Documentos/Facturas/2026 y renómbralo con la fecha"*— y Tidyfile vigila tus carpetas y las mantiene ordenadas por sí solo.

Todo funciona en local. Sin nube, sin telemetría, sin suscripción.

> **Estado: pre-lanzamiento.** La aplicación funciona de principio a fin —reglas, vista previa, ordenado, deshacer y vigilancia en tiempo real—, pero todavía no ha pasado por una beta y no hay versiones publicadas. Si quieres probarla, compílala desde el código fuente.

## Por qué

A todas las herramientas que existen les falta algo:

| | Precio | Multiplataforma | Interfaz gráfica | Código abierto | Mantenida |
|---|---|---|---|---|---|
| Hazel | 42 $ | Solo macOS | Sí | No | Sí |
| File Juggler | ~40 $ | Solo Windows | Sí | No | Sí |
| File Arbor | Freemium | Win / macOS | Sí | No | Sí |
| DropIt | Gratis | Solo Windows | Sí | Sí | Sin versiones desde 2018 |
| organize | Gratis | Sí | No — CLI + YAML | Sí | Sí |
| hazelnut | Gratis | Sí | No — interfaz de terminal | Sí | Sí |
| **Tidyfile** | **Gratis** | **Win / macOS / Linux** | **Sí** | **Sí** | **Sí** |

Ninguna otra marca todas las casillas, y ninguna otra ofrece una interfaz gráfica de verdad en Linux.

## Tus archivos son el producto

Un organizador de archivos que pierde los archivos de alguien no vale nada, así que aquí la seguridad no es una función más: es el diseño.

- **Nunca se borra nada.** Lo que se elimina va a la papelera del sistema. No hay borrado directo en ningún punto del código, y la batería de tests lo impone.
- **Vista previa antes de mover nada.** Cada regla muestra exactamente qué archivos tocaría y qué pasaría con cada uno. La vista previa y la ejecución real comparten el mismo código, así que no pueden contradecirse.
- **Todo se puede deshacer.** Cada operación se escribe en un journal SQLite *antes* de tocar el disco, y después se marca como hecha. Un clic revierte un lote entero, incluidos los aplicados automáticamente mientras se vigilaba una carpeta.
- **Nada se sobrescribe.** Si un nombre ya existe, se añade un sufijo numerado. Nunca se reemplaza un archivo existente.
- **Las interrupciones se sobreviven.** Si la aplicación se interrumpe a la fuerza a mitad de una ejecución, el journal registra lo que quedó sin confirmar.

## Funciones

- **Editor visual de reglas** — condiciones combinables (todas/alguna) y acciones. Sin YAML y sin regex obligatorias.
- **Condiciones** — extensión, el nombre contiene, glob, regex, tamaño, antigüedad, subcarpeta.
- **Acciones** — mover, copiar, renombrar con plantillas, enviar a la papelera y subcarpetas automáticas por fecha o tipo.
- **Vigilancia en tiempo real** — los archivos nuevos se ordenan al llegar, con un margen de espera para no tocar las descargas a medias.
- **Historial** — cada lote aparece en la lista, y cada lote se puede deshacer.

Las plantillas de renombrado y de subcarpetas aceptan `{name}`, `{ext}`, `{date}`, `{year}`, `{month}`, `{day}` y `{counter}`.

## Compilar desde el código fuente

Necesitas [Rust](https://rustup.rs) 1.85+ y [Node.js](https://nodejs.org) 22+, además de los [requisitos de Tauri](https://tauri.app/start/prerequisites/) para tu plataforma.

```bash
npm install
npm run tauri dev      # ejecutar en desarrollo
npm run tauri build    # generar los instaladores
```

## Versiones sin firmar

Las versiones publicadas no están firmadas digitalmente: un certificado de Apple cuesta 99 $ al año y uno de Windows más, algo que hoy no es sostenible para un proyecto gratuito.

Por eso macOS y Windows te avisarán la primera vez que abras la aplicación. Las sumas SHA-256 publicadas te permiten comprobar que lo que descargaste coincide con lo que compiló la integración continua. La firma puede llegar más adelante si el proyecto lo justifica.

## Licencia

[GNU General Public License v3.0](LICENSE).

Tidyfile existe porque las buenas herramientas de esta categoría son de pago y cerradas. La GPL garantiza lo contrario: cualquiera puede usarlo, estudiarlo, modificarlo y compartirlo, pero un derivado que se distribuya tiene que entregarse con su código fuente y bajo las mismas condiciones. Nadie puede cerrarlo y vendértelo de vuelta.
