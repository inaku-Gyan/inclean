#ifndef INC_FREERTOS_H
#define INC_FREERTOS_H

#include <stddef.h>

#include <stdint.h> /* READ COMMENT ABOVE. */

#ifdef __cplusplus
extern "C" {
#endif

/* Application specific configuration options. */
#include "FreeRTOSConfig.h"

/* Basic FreeRTOS definitions. */
#include "projdefs.h"

/* Definitions specific to the port being used. */
#include "portable.h"

/* Must be defaulted before configUSE_NEWLIB_REENTRANT is used below. */
#ifndef configUSE_NEWLIB_REENTRANT
	#define configUSE_NEWLIB_REENTRANT 0
#endif

/* Required if struct _reent is used. */
#if ( configUSE_NEWLIB_REENTRANT == 1 )
	#include <reent.h>
#endif

#ifdef __cplusplus
}
#endif

#endif /* INC_FREERTOS_H */

