<script setup lang="ts">
const props = defineProps<{ name: string }>()
const greeting = `Hello, ${props.name}!`
</script>

<template>
  <p class="greeting">{{ greeting }}</p>
</template>

<style scoped>
.greeting {
  font-weight: 600;
}
</style>
