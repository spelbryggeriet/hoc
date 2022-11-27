#!/bin/sh

set -u

main() {
  local local_bin_dir="$HOME/.local/bin"
  
  # Check if the command is already installed.
  if test -e "$local_bin_dir/hoc"; then
    # Ask for approval to overwrite the existing command.
    read -p '`hoc` is already installed in "~/.local/bin". Do you want to overwrite it? [Y/n] ' input </dev/tty
    case `echo "$input"  | tr '[:upper:]' '[:lower:]'` in
      n)
        echo "Aborting"
        return 1
        ;;
    esac

    # Remove the file.
    rm "$local_bin_dir/hoc"
  fi

  # Create local binary directory if it does not already exist.
  mkdir -p $local_bin_dir

  # Update PATH, if needed.
  case ":${PATH}:" in
    # The path is included in the PATH variable.
    *":${local_bin_dir}:"*)
      ;;
    # The path is not included, so we update the PATH variable.
    *)
      echo "export PATH=\"${PATH}:${local_bin_dir}\"" >> "$HOME/.profile"
      ;;
  esac

  echo '=> Downloading `hoc`'
  
  # Fetch the latest version number.
  local version=`curl --proto '=https' --tlsv1.2 -sSf \
    https://api.github.com/repos/spelbryggeriet/hoc/releases/latest \
    | sed -n 's/^  "tag_name": "\([^"]*\)",$/\1/p'`
  local filename="hoc_macos-x86_64_${version}.zip"

  # Create temporary directory.
  local tmp_dir=/tmp/hoc
  mkdir -p "$tmp_dir"

  # Fetch the executable archive.
  curl --proto '=https' --tlsv1.2 -sSfL -o "$tmp_dir/$filename" \
    https://github.com/spelbryggeriet/hoc/releases/download/$version/$filename

  echo '=> Installing `hoc`'

  # Extract the executable into the local bin directory.
  unzip "$tmp_dir/$filename" -d "$local_bin_dir" >/dev/null

  # Remove the temporary directory.
  rm -r "$tmp_dir"

  echo '`hoc` installed'
}

main || exit 1
