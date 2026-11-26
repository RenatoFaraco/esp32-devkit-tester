# Top-level ESP-IDF project for the on-target test firmware.
# One build per chip: idf.py -B build-<target> -D SDKCONFIG=build-<target>/sdkconfig set-target <target> build
cmake_minimum_required(VERSION 3.16)

include($ENV{IDF_PATH}/tools/cmake/project.cmake)

# Version pin (ADR-001). The IDF itself is installed outside the repo, so this guard is how the
# project fixes the version it was validated against; CI uses the espressif/idf:v5.5.2 image.
# IDF_VERSION_MAJOR/MINOR/PATCH are defined by project.cmake above.
set(DKT_IDF_VERSION "${IDF_VERSION_MAJOR}.${IDF_VERSION_MINOR}.${IDF_VERSION_PATCH}")
if(DKT_IDF_VERSION VERSION_LESS "5.5.2")
    message(FATAL_ERROR
        "ESP-IDF ${DKT_IDF_VERSION} found, but this project requires >= 5.5.2 "
        "(ESP32-C5 needs >= 5.5.1 and ESP32-P4 rev3 needs >= 5.5.2).")
endif()
if(NOT (IDF_VERSION_MAJOR EQUAL 5 AND IDF_VERSION_MINOR EQUAL 5))
    message(WARNING
        "ESP-IDF ${DKT_IDF_VERSION} found; this project is validated on 5.5.x (pinned 5.5.2).")
endif()

project(devkit_tester_fw)
