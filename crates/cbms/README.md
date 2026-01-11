# CBOR-based Message Stream (CBMS)

**CBMS** is a high-performance, language-agnostic messaging protocol. It provides a consistent interface for structured communication between processes or services, supporting request/response patterns, one-way notifications, and continuous streaming of data. Messages are encoded in CBOR, which is fast and compact, and can be converted to JSON for human-readable debugging or logging.

CBMS is designed to be transport-agnostic. It can operate over standard input/output, SSH tunnels, TCP connections, or even high-speed UDP channels. The protocol itself does not handle authentication or authorization. Authentication should be provided by the transport layer, for example through SSH, and authorization can rely on filesystem permissions. This allows CBMS to remain focused on message semantics and integrity, without being tied to identity management.

This crate offers a pure Rust implementation of CBMS.

## License

CBOR-based Message Stream (CBMS) Rust implementation

Copyright (C) 2026  Julian Siebert

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but **WITHOUT ANY WARRANTY**; without even the implied warranty of
**MERCHANTABILITY** or **FITNESS FOR A PARTICULAR PURPOSE**. See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program. If not, see <https://www.gnu.org/licenses/>.
