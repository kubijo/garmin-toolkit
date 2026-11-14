import 'common.just'

cache_tool := repository_root / "infra/just/build-cache.sh"

# Report repository build caches and read-only external cache totals.
report:
    @bash {{ quote(cache_tool) }} report

# Clean one named target cache; preview unless MODE is explicitly apply.
clean CACHE MODE='dry-run':
    @bash {{ quote(cache_tool) }} clean {{ quote(CACHE) }} {{ quote(MODE) }}
