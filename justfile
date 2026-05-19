# List available recipes.
default:
    @just --list

# Build the release binary.
build:
    @bash "{{justfile_directory()}}/just/macos-service.sh" build

# Build and overwrite deploy as a macOS LaunchDaemon.
macos-deploy:
    @bash "{{justfile_directory()}}/just/macos-service.sh" deploy

# Start the macOS service.
macos-start:
    @bash "{{justfile_directory()}}/just/macos-service.sh" start

# Stop the macOS service.
macos-stop:
    @bash "{{justfile_directory()}}/just/macos-service.sh" stop

# Restart the macOS service.
macos-restart:
    @bash "{{justfile_directory()}}/just/macos-service.sh" restart

# Print macOS service status.
macos-status:
    @bash "{{justfile_directory()}}/just/macos-service.sh" status

# Show recent service logs. Usage: just macos-logs 200
macos-logs lines="100":
    @LINES="{{lines}}" bash "{{justfile_directory()}}/just/macos-service.sh" logs

# Follow service logs. Usage: just macos-follow 200
macos-follow lines="100":
    @LINES="{{lines}}" bash "{{justfile_directory()}}/just/macos-service.sh" follow

# Probe the local health endpoint.
macos-health:
    @bash "{{justfile_directory()}}/just/macos-service.sh" health

# Remove the LaunchDaemon registration and plist, keeping installed files.
macos-uninstall:
    @bash "{{justfile_directory()}}/just/macos-service.sh" uninstall
