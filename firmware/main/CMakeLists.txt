idf_component_register(SRCS "main.c")

# IDF 5.5 already compiles C with -std=gnu17 (C17 plus the GNU extensions its headers rely on).
#
# CONVENTIONS §3 asks for -Wall -Wextra -Werror, but the IDF's global flags (applied before ours)
# relax that for every component: -Wno-unused-parameter, -Wno-sign-compare, -Wno-enum-conversion
# and -Wno-error=unused-function/unused-variable/unused-but-set-variable/deprecated-declarations/
# extra. A plain -Werror does NOT undo a specific -Wno-error=<x>, and CMake drops our -Wall/-Wextra
# as duplicates. So the warnings the IDF switched off are re-enabled one by one, as errors, for
# our code only (the IDF's own components keep their flags). -Wvla enforces "no VLA".
target_compile_options(${COMPONENT_LIB} PRIVATE
    -Wall -Wextra -Werror -Wvla
    -Werror=unused-parameter
    -Werror=sign-compare
    -Werror=enum-conversion
    -Werror=unused-function
    -Werror=unused-variable
    -Werror=unused-but-set-variable
    -Werror=deprecated-declarations
    -Werror=extra
)
