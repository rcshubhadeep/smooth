#!/usr/bin/env python3

import argparse
import json
import os
import sys
from pathlib import Path
from datetime import datetime

def get_cache_dir():
    """Determine the appropriate cache directory based on OS."""
    if sys.platform == "darwin":  # macOS
        return Path.home() / "Library" / "Caches" / "llama.cpp"
    elif sys.platform.startswith("linux"):  # Linux
        return Path.home() / ".cache" / "llama.cpp"
    else:
        raise NotImplementedError("Unsupported platform")

def construct_model_name_from_manifest_name(manifest_name):
    """Convert manifest filename to model name."""
    # Example: manifest=unsloth_Qwen3-30B-A3B-GGUF=Q4_K_XL.json
    if manifest_name.startswith("manifest=") and manifest_name.endswith(".json"):
        # Extract the part between first '=' and second '='
        parts = manifest_name[9:-5].split("=")
        if len(parts) >= 2:
            # Replace only the first underscore with slash
            path_part = parts[0]
            if "_" in path_part:
                path_part = path_part.replace("_", "/", 1)
            return ":".join([path_part, parts[1]])
    return "unknown"

def get_gguf_filename_from_manifest(manifest):
    """Extract GGUF filename from manifest."""
    try:
        gguf_file = manifest.get("ggufFile", {})
        rfilename = gguf_file.get("rfilename", "")
        if rfilename.endswith(".gguf"):
            return rfilename
        return ""
    except Exception:
        return ""

def find_related_files(cache_dir, manifest_filename):
    """Find all files related to a specific manifest file."""
    files = set()

    # The manifest file is already known, so we look for related files
    manifest_file = cache_dir / manifest_filename
    if manifest_file.exists():
        files.add(manifest_file)

        model_prefix = manifest_filename.split('=')[1]

        # Read manifest to find GGUF file and related metadata
        with open(manifest_file, "r") as f:
            manifest = json.load(f)
        gguf_file_name = "_".join([model_prefix, manifest['ggufFile']['rfilename']])
        gguf_file = cache_dir / gguf_file_name
        if gguf_file.exists():
            files.add(gguf_file)

        # Find corresponding meta file (.etag or .json)
        etag_file = gguf_file.with_suffix('.gguf.etag')
        json_file = gguf_file.with_suffix('.gguf.json')

        if etag_file.exists():
            files.add(etag_file)
        if json_file.exists():
            files.add(json_file)

        if 'mmprojFile' in manifest:
            mmproj_file_name = "_".join([model_prefix, manifest['mmprojFile']['rfilename']])
            gguf_file = cache_dir / mmproj_file_name
            if gguf_file.exists():
                files.add(gguf_file)

            # Find corresponding meta file (.etag or .json)
            etag_file = gguf_file.with_suffix('.gguf.etag')
            json_file = gguf_file.with_suffix('.gguf.json')

            if etag_file.exists():
                files.add(etag_file)
            if json_file.exists():
                files.add(json_file)

    return files

def list_models(cache_dir):
    """List all cached models."""
    cache_dir = Path(cache_dir)
    if not cache_dir.exists():
        print("Cache directory does not exist.")
        return

    # Collect all manifest files first
    manifest_files = []
    for file in cache_dir.iterdir():
        if file.is_file() and file.name.startswith("manifest=") and file.name.endswith(".json"):
            manifest_files.append(file)

    if not manifest_files:
        print("No cached models found.")
        return

    models = {}

    for manifest_file in sorted(manifest_files):
        # Get the model name from manifest filename
        model_name = construct_model_name_from_manifest_name(manifest_file.name)

        # Get all related files for this model using manifest file name
        related_files = find_related_files(cache_dir, manifest_file.name)

        # Calculate total size
        total_size = sum(f.stat().st_size for f in related_files)

        # Get modification time from manifest
        modified = datetime.fromtimestamp(manifest_file.stat().st_mtime)

        models[model_name] = {
                "size": total_size,
                "modified": modified,
                "files": related_files
                }

    # Print table with 80 column width
    print(f"{'Name':<60}{'Size (GB)':<10}{'Modified'}")
    print("-" * 80)
    for model_name, data in models.items():
        size_mb = round(data["size"] / (1024 * 1024 * 1024), 1)
        modified = data["modified"].strftime("%Y-%m-%d %H:%M")
        print(f"{model_name:<60}{size_mb:<10}{modified}")

def remove_model(cache_dir, model_name):
    """Delete a specific model by name, along with manifest and metadata."""
    cache_dir = Path(cache_dir)
    if not cache_dir.exists():
        print("Cache directory does not exist.")
        return

    # Collect all manifest files and look for one matching the model name
    manifest_files = []
    for file in cache_dir.iterdir():
        if file.is_file() and file.name.startswith("manifest=") and file.name.endswith(".json"):
            manifest_files.append(file)

    found_manifest = None
    for manifest_file in manifest_files:
        manifest_name = manifest_file.name
        if construct_model_name_from_manifest_name(manifest_name) == model_name:
            found_manifest = manifest_name
            break

    if not found_manifest:
        print(f"Model {model_name} not found.")
        return

    # Find all related files for this manifest file
    related_files = find_related_files(cache_dir, found_manifest)

    if not related_files:
        print(f"Model {model_name} not found.")
        return

    # Delete all related files
    for file in related_files:
        try:
            file.unlink()
            print(f"Deleted: {file}")
        except Exception as e:
            print(f"Failed to delete {file}: {e}")

def main():
    parser = argparse.ArgumentParser(prog=sys.argv[0])
    subparsers = parser.add_subparsers(dest="command", required=True, help="Subcommands")

    # ls subcommand
    ls_parser = subparsers.add_parser("ls", help="List cached models")
    ls_parser.set_defaults(func=list_models)

    # rm subcommand
    rm_parser = subparsers.add_parser("rm", help="Remove a cached model")
    rm_parser.add_argument("model_name", help="Name of the model to remove")
    rm_parser.set_defaults(func=remove_model)

    args = parser.parse_args()
    cache_dir = get_cache_dir()

    if args.command == "ls":
        list_models(cache_dir)
    elif args.command == "rm":
        remove_model(cache_dir, args.model_name)

if __name__ == "__main__":
    main()
