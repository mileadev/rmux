inventory_collected_entries=()

resolve_without_symlinks() {
  local path label absolute component candidate last_index
  local -a raw_components=()
  local -a resolved_components=()
  path="$1"
  label="$2"
  case "$path" in
    *$'\n'*|*$'\r'*) die "$label path contains a line break" ;;
  esac
  case "$path" in
    /*) absolute="$path" ;;
    *) absolute="$(pwd -P)/$path" ;;
  esac

  IFS=/ read -r -a raw_components <<< "$absolute"
  for component in "${raw_components[@]}"; do
    case "$component" in
      ""|.) continue ;;
      ..)
        if [ "${#resolved_components[@]}" -gt 0 ]; then
          last_index=$((${#resolved_components[@]} - 1))
          unset "resolved_components[$last_index]"
        fi
        continue
        ;;
    esac

    resolved_components+=("$component")
    candidate=
    for component in "${resolved_components[@]}"; do
      candidate="$candidate/$component"
    done
    [ ! -L "$candidate" ] ||
      die "$label must not traverse symbolic links: $path"
  done

  candidate=
  for component in "${resolved_components[@]}"; do
    candidate="$candidate/$component"
  done
  printf '%s\n' "${candidate:-/}"
}

paths_overlap() {
  case "$1" in "$2"|"$2"/*) return 0 ;; esac
  case "$2" in "$1"|"$1"/*) return 0 ;; esac
  return 1
}

collect_find_entries() {
  local label inventory_file entry
  label="$1"
  shift
  inventory_collected_entries=()

  inventory_file="$(mktemp "${TMPDIR:-/tmp}/rmux-apt-find.XXXXXX")" ||
    die "cannot create temporary inventory for $label"
  if ! find "$@" -print0 > "$inventory_file"; then
    rm -f -- "$inventory_file"
    die "cannot enumerate $label"
  fi
  while IFS= read -r -d '' entry; do
    inventory_collected_entries+=("$entry")
  done < "$inventory_file"
  rm -f -- "$inventory_file"
}

collect_sorted_find_entries() {
  local label inventory_file entry
  local unsorted=()
  label="$1"
  shift
  collect_find_entries "$label" "$@"
  unsorted=("${inventory_collected_entries[@]}")
  inventory_collected_entries=()
  [ "${#unsorted[@]}" -gt 0 ] || return 0

  inventory_file="$(mktemp "${TMPDIR:-/tmp}/rmux-apt-sort.XXXXXX")" ||
    die "cannot create temporary inventory for $label"
  if ! printf '%s\0' "${unsorted[@]}" | LC_ALL=C sort -z > "$inventory_file"; then
    rm -f -- "$inventory_file"
    die "cannot sort $label"
  fi
  while IFS= read -r -d '' entry; do
    inventory_collected_entries+=("$entry")
  done < "$inventory_file"
  rm -f -- "$inventory_file"
}

inventory_tree_entries() {
  local root label entry
  root="$1"
  label="$2"

  [ -d "$root" ] && [ ! -L "$root" ] ||
    die "$label root is missing or unsafe"
  collect_find_entries "$label" -P "$root" -mindepth 1
  for entry in "${inventory_collected_entries[@]}"; do
    if [ -L "$entry" ]; then
      readlink -- "$entry" >/dev/null ||
        die "cannot read symbolic link in $label"
      die "$label must not contain symbolic links"
    elif [ -f "$entry" ]; then
      sha256sum -- "$entry" >/dev/null ||
        die "cannot read regular file in $label"
    elif [ ! -d "$entry" ]; then
      die "$label contains an unsupported entry type: $entry"
    fi
  done
}

validate_apt_package_identity() {
  local package expected_architecture package_name package_architecture
  package="$1"
  expected_architecture="$2"
  dpkg-deb -f "$package" >/dev/null ||
    die "cannot read APT package metadata: $package"
  package_name="$(dpkg-deb -f "$package" Package)" ||
    die "cannot read APT Package metadata: $package"
  [ "$package_name" = rmux ] ||
    die "APT Package identity must be rmux: $package"
  package_architecture="$(dpkg-deb -f "$package" Architecture)" ||
    die "cannot read APT Architecture metadata: $package"
  [ "$package_architecture" = "$expected_architecture" ] ||
    die "APT Architecture metadata does not match $expected_architecture: $package"
}

validate_input_packages() {
  local architecture deb matches
  input_packages=()
  shopt -s nullglob
  for architecture in "${architectures[@]}"; do
    matches=("$input_dir"/rmux_*_"$architecture".deb)
    [ "${#matches[@]}" -gt 0 ] ||
      die "no rmux_*_${architecture}.deb files found in $input_dir"
    for deb in "${matches[@]}"; do
      [ -f "$deb" ] && [ ! -L "$deb" ] ||
        die "unsafe APT input package: $deb"
      case "${deb##*/}" in *$'\n'*) die "APT input package name contains a newline" ;; esac
      validate_apt_package_identity "$deb" "$architecture"
      input_packages+=("$deb")
    done
  done
  shopt -u nullglob
}
