cmake_minimum_required(VERSION 3.15)
project(driver-installer C)

set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)

# Unicode 支持
add_definitions(-DUNICODE -D_UNICODE)

# 窗口程序（无控制台）
set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} /SUBSYSTEM:WINDOWS")

# 输出目录
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_SOURCE_DIR}/bin)

add_executable(driver-installer
    src/driver-installer.c
)

# 附加库
target_link_libraries(driver-installer
    advapi32
    kernel32
    user32
)

# 安装规则（可选）
install(TARGETS driver-installer DESTINATION bin)
