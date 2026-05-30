#ifndef PORTABLE_H
#define PORTABLE_H

#include "deprecated_definitions.h"  // IWYU: export

#ifndef portENTER_CRITICAL
	#include "../portable/GCC/ARM_CM4F/portmacro.h"  // IWYU: export
#endif

#ifdef __cplusplus
extern "C" {
#endif

#include "mpu_wrappers.h"  // IWYU: export

#ifdef __cplusplus
}
#endif

#endif /* PORTABLE_H */
