/* closed prefix */ #include "lib/alpha.h" // IWYU pragma: export
int x; /* closed */ #include "not_real.h"
#/**/include /* gap */ "lib/delta.h" // IWYU pragma: export
#include/* gap */"lib/epsilon.h" // IWYU pragma: export
#include "lib/beta.h" /* this comment
#include "hidden.h"
spans two lines */
#include "lib/zeta.h" /* closed */ /* opens
#include "hidden3.h"
spans after a closed block */
#include "lib/gamma.h" /* never closes
#include "hidden2.h"
