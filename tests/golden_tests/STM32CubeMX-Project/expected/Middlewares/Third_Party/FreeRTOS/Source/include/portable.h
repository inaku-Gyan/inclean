#ifndef PORTABLE_H
#define PORTABLE_H

#include "deprecated_definitions.h"

#ifndef portENTER_CRITICAL
	#include "../portable/GCC/ARM_CM4F/portmacro.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif

#include "mpu_wrappers.h"

#ifdef __cplusplus
}
#endif

#endif /* PORTABLE_H */
