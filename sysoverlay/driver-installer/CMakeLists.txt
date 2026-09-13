cmake_minimum_required(VERSION 3.15)
project(driver-installer C)

set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)

# Unicode support
add_definitions(-DUNICODE -D_UNICODE)

# Window program (no console)
set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} /SUBSYSTEM:WINDOWS")

# Output directory
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_SOURCE_DIR}/bin)

# --- Embed WinRing0x64.sys as a resource ---
# The driver binary must be present at configure time; it gets baked into the exe.
set(SYS_BIN "${CMAKE_CURRENT_SOURCE_DIR}/WinRing0x64.sys")
set(MANIFEST_PATH "${CMAKE_CURRENT_SOURCE_DIR}/driver-installer.manifest")
if(EXISTS "${SYS_BIN}")
    # Generate a .rc that points at the sys + manifest with absolute paths, so the
    # resource compiler finds them regardless of the working directory.
    configure_file(
        "${CMAKE_CURRENT_SOURCE_DIR}/driver-installer.rc.in"
        "${CMAKE_CURRENT_BINARY_DIR}/driver-installer_gen.rc"
        @ONLY)
    set(DRIVER_RC "${CMAKE_CURRENT_BINARY_DIR}/driver-installer_gen.rc")
else()
    message(WARNING
        "WinRing0x64.sys not found at ${SYS_BIN}.\n"
        "  Place it next to CMakeLists.txt, then re-run cmake.\n"
        "  (build.bat is the recommended build path and also embeds the driver.)")
    set(DRIVER_RC "")
endif()

add_executable(driver-installer
    src/driver-installer.c
    ${DRIVER_RC}
)

# Link libraries
target_link_libraries(driver-installer
    advapi32
    kernel32
    user32
    shell32
)

# Install rule (optional)
install(TARGETS driver-installer DESTINATION bin)
