import unittest

from natural_cohort_lab import host_cpu_values


class HostCpuAccounting(unittest.TestCase):
    def test_excludes_idle_wait_steal_and_duplicate_guest_time(self):
        value = host_cpu_values("cpu 100 20 30 400 50 6 7 8 9 10", 100)
        self.assertAlmostEqual(value["busy_seconds"], 1.63)
        self.assertAlmostEqual(value["softirq_seconds"], .07)
        self.assertAlmostEqual(value["steal_seconds"], .08)

    def test_uses_reported_clock_and_rejects_wrong_row(self):
        self.assertAlmostEqual(host_cpu_values("cpu 10 2 3 100 50 1 1 4 2 1", 10)["busy_seconds"], 1.7)
        with self.assertRaises(ValueError):
            host_cpu_values("cpu0 10 2 3 100 50 1 1 4 2 1", 10)
        with self.assertRaises(ValueError):
            host_cpu_values("cpu 10", 100)


if __name__ == "__main__":
    unittest.main()
