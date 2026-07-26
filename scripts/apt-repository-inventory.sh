resolve_without_symlinks() {
  local path label logical physical
  path="$1"
  label="$2"
  logical="$(realpath -ms -- "$path")" ||
    die "cannot resolve $label: $path"
  physical="$(realpath -m -- "$path")" ||
    die "cannot resolve $label: $path"
  [ "$logical" = "$physical" ] ||
    die "$label must not traverse symbolic links: $path"
  printf '%s\n' "$logical"
}

paths_overlap() {
  case "$1" in "$2"|"$2"/*) return 0 ;; esac
  case "$2" in "$1"|"$1"/*) return 0 ;; esac
  return 1
}

collect_find_entries() {
  local destination_name label find_fd find_pid entry
  destination_name="$1"
  label="$2"
  shift 2
  local -n destination="$destination_name"
  destination=()

  exec {find_fd}< <(find "$@" -print0)
  find_pid=$!
  while IFS= read -r -d '' entry <&"$find_fd"; do
    destination+=("$entry")
  done
  exec {find_fd}<&-
  wait "$find_pid" || die "cannot enumerate $label"
}

collect_sorted_find_entries() {
  local destination_name label sort_fd sort_pid entry
  local unsorted=()
  destination_name="$1"
  label="$2"
  shift 2
  local -n destination="$destination_name"
  collect_find_entries unsorted "$label" "$@"
  destination=()
  [ "${#unsorted[@]}" -gt 0 ] || return 0

  exec {sort_fd}< <(printf '%s\0' "${unsorted[@]}" | LC_ALL=C sort -z)
  sort_pid=$!
  while IFS= read -r -d '' entry <&"$sort_fd"; do
    destination+=("$entry")
  done
  exec {sort_fd}<&-
  wait "$sort_pid" || die "cannot sort $label"
}

inventory_tree_entries() {
  local destination_name root label entry metadata
  destination_name="$1"
  root="$2"
  label="$3"
  local -n destination="$destination_name"

  stat -c '%F	%a	%s' -- "$root" >/dev/null ||
    die "cannot stat $label root"
  collect_find_entries "$destination_name" "$label" -P "$root" -mindepth 1
  for entry in "${destination[@]}"; do
    metadata="$(stat -c '%F	%a	%s' -- "$entry")" ||
      die "cannot stat entry in $label"
    if [ -L "$entry" ]; then
      readlink -- "$entry" >/dev/null ||
        die "cannot read symbolic link in $label"
      die "$label must not contain symbolic links"
    elif [ -f "$entry" ]; then
      sha256sum -- "$entry" >/dev/null ||
        die "cannot read regular file in $label"
    elif [ ! -d "$entry" ]; then
      die "$label contains an unsupported entry type: $metadata"
    fi
  done
}
