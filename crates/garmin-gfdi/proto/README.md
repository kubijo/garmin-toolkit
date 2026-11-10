# FileAccess schemas

`garmin/gdi_file_access.proto` and `garmin/gdi_data_types.proto` are Buf-formatted copies from
`GBProtobufGenerator/src/main/proto/garmin/` at
[Gadgetbridge revision 28cea5443cfa35ff1dfe047645e7abf18ced24cc][source], under AGPL-3.0-or-later. Java-only
code-generation options are removed; wire definitions are unchanged.

`toolkit_file_access.proto` projects only field 43 of upstream `gdi_smart_proto.proto`, preserving its package, message
name, field number, and type. Other Smart services are intentionally absent. This is a Protobuf message envelope, not an
HTTP/gRPC service definition.

Buffa 0.9.2 generates Rust messages during Cargo builds using the Nix-provided `protoc`. Build output is not checked in;
no `.tmp` checkout or network access is needed for generation. Buf owns formatting and linting through the repository
format and preflight commands. `buf.yaml` scopes naming and required-field exceptions to the imported proto2 contracts;
package exceptions also cover the matching Smart projection. Field names, numbers, types, and defaults stay
upstream-compatible.

SHA-256 of the original upstream files before Buf formatting:

```text
3659b43b8dfeb1df7b1d8939be5b0a2eeae193c4313baa9e22ae8c0ec6dd0402  garmin/gdi_file_access.proto
29ed3cfa476dc3449fc4f1b0d938912f9e602e003388e48c4d1838e428310774  garmin/gdi_data_types.proto
```

The full schema includes deletion and flag mutation messages. Generated types describe the wire format; the download
client must expose only its audited listing, pull, flow-control, and owned-transfer cleanup operations.

[source]: https://codeberg.org/Freeyourgadget/Gadgetbridge/tree/28cea5443cfa35ff1dfe047645e7abf18ced24cc/GBProtobufGenerator/src/main/proto/garmin
